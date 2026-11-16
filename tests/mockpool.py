#!/usr/bin/env python3
# FreeForGem - see LICENSE
"""Mock pools for the failover / failback integration test (tests/failback_test.sh), localhost only.

  mockpool.py --mode pearl|noid|prox|innov|cmfd --port N --events FILE [--tls CERT KEY]

One process = one pool; the test starts and kills them to take a pool down and bring it back.  Every event is one line in FILE:
  <unix time> login <wallet / login string>     the login of a connection (user and fee are told apart by the wallet)
  <unix time> job <job id>                      a job sent
  <unix time> share <login> <job id>            a share received (and accepted)
  <unix time> close                             a connection ended
Every submitted share is accepted (the miner re-computes every candidate itself); --lag S delays the answers (shares in flight).
Modes: pearl (JSON-RPC stratum with 76-byte header jobs), noid (parano1d-stratum-v1, the job recorded from a live pool:
tests/fixtures/noid-job.json), prox (suprnova-format stratum, hash jobs), innov (InnovLab JSON-lines), cmfd (TLS 1.3 with a pinned
certificate, 4-byte length + JSON frames: login challenge, hello, ack, job; enough for the miner's failback check; the real engine
cannot mine against it).  --tls CERT KEY: TLS (noid / prox / innov over stratum+ssl://).
"""
import argparse, json, os, random, socket, ssl, struct, sys, threading, time

ap = argparse.ArgumentParser()
ap.add_argument('--mode', required=True, choices=['pearl', 'noid', 'prox', 'innov', 'cmfd'])
ap.add_argument('--port', type=int, required=True)
ap.add_argument('--events', required=True)
ap.add_argument('--tls', nargs=2, metavar=('CERT', 'KEY'))
ap.add_argument('--lag', type=float, default=0.15, help='seconds before a share is answered')
ap.add_argument('--interval', type=float, default=8.0, help='seconds between jobs')
ap.add_argument('--submit-mode', default='accept', choices=['accept', 'reject', 'silent'], help='what a share gets: credited, rejected, or no answer at all (a pool that credits nothing)')
ap.add_argument('--mute-after', type=float, default=0.0, help='seconds after the pool started it dies half-open: connections that are open stay open but nothing is sent any more (what a dead route / DROP rule makes) and no new connection is accepted (the port closes); 0 = never')
ap.add_argument('--k', type=int, default=30, help='prox / innov share difficulty (zero low bits)')
ap.add_argument('--difficulty', type=float, default=100000.0, help='pearl share difficulty')
ap.add_argument('--noid-hi', default='0000000040000000', help='noid share target, bytes 24..31 (hex, little endian as sent)')
a = ap.parse_args()

START = time.time()
EV = open(a.events, 'a', buffering=1)
evl = threading.Lock()


def ev(kind, detail=''):
    with evl:
        EV.write('%.3f %s %s\n' % (time.time(), kind, detail))


class Conn:
    def __init__(self, s):
        self.s, self.wl, self.alive = s, threading.Lock(), True
        self.login = ''
        self.mute_logged = False

    def muted(self):
        if a.mute_after > 0 and time.time() - START >= a.mute_after:
            if not self.mute_logged:
                self.mute_logged = True
                ev('mute-conn')
            return True
        return False

    def send_line(self, o):
        if self.muted():
            return
        try:
            with self.wl:
                self.s.sendall((json.dumps(o, separators=(',', ':')) + '\n').encode())
        except OSError:
            self.alive = False

    def send_frame(self, o):
        if self.muted():
            return
        b = json.dumps(o, separators=(',', ':')).encode()
        try:
            with self.wl:
                self.s.sendall(struct.pack('>I', len(b)) + b)
        except OSError:
            self.alive = False

    def later(self, o, bad=None):
        if a.submit_mode == 'silent':
            return
        if a.submit_mode == 'reject' and bad is not None:
            o = bad
        def run():
            time.sleep(a.lag)
            self.send_line(o)
        threading.Thread(target=run, daemon=True).start()


def lines(c):
    buf = b''
    while True:
        try:
            d = c.s.recv(65536)
        except OSError:
            return
        if not d:
            return
        buf += d
        while b'\n' in buf:
            l, buf = buf.split(b'\n', 1)
            if l.strip():
                try:
                    yield json.loads(l)
                except ValueError:
                    pass


def every(c, make_job, stop_at_close=True):
    """a job now and then (clean) until the connection ends"""
    def run():
        n = 0
        while c.alive:
            n += 1
            make_job(c, n)
            end = time.time() + a.interval
            while c.alive and time.time() < end:
                time.sleep(0.1)
    threading.Thread(target=run, daemon=True).start()


# ---------------------------------------------------------------- pearl
def pearl(c):
    for o in lines(c):
        m = o.get('method')
        if m == 'mining.subscribe':
            c.send_line({'jsonrpc': '2.0', 'id': o['id'], 'result': []})
        elif m == 'mining.authorize':
            p = o.get('params') or {}
            c.login = '%s' % (p.get('wallet') if isinstance(p, dict) else p[0])
            ev('login', c.login)
            c.send_line({'jsonrpc': '2.0', 'id': o['id'], 'result': True})

            def job(cc, n):
                rnd = random.Random(n * 7919 + a.port)
                hdr = bytes(rnd.getrandbits(8) for _ in range(76)).hex()
                jid = 'p%d_%d' % (a.port, n)
                cc.send_line({'jsonrpc': '2.0', 'method': 'mining.notify', 'params': {'job_id': jid, 'header': hdr, 'difficulty': a.difficulty}})
                ev('job', jid)
            every(c, job)
        elif m == 'mining.submit':
            p = o.get('params')
            jid = p.get('job_id') if isinstance(p, dict) else (p or ['?'])[0]
            ev('share', '%s %s' % (c.login, jid))
            c.later({'jsonrpc': '2.0', 'id': o['id'], 'result': True}, {'jsonrpc': '2.0', 'id': o['id'], 'result': None, 'error': {'code': 20, 'message': 'rejected'}})
        elif 'id' in o:  # an unknown method: "method not supported" (what the real pool answers)
            ev('unknown', str(m))
            c.send_line({'jsonrpc': '2.0', 'id': o['id'], 'result': None, 'error': {'code': -32601, 'message': 'method not supported'}})


# ---------------------------------------------------------------- noid
FIX = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fixtures', 'noid-job.json')))


def noid(c):
    for o in lines(c):
        m = o.get('method')
        if m == 'mining.subscribe':
            c.send_line({'id': o['id'], 'result': FIX['subscribe_result'], 'error': None})
        elif m == 'mining.authorize':
            c.login = '%s' % o['params'][0]
            ev('login', c.login)
            c.send_line({'id': o['id'], 'result': True, 'error': None})

            def job(cc, n):
                j = dict(FIX['job'])
                j['job_id'] = '%032x' % (0xabc000 + n + a.port * 1000)
                j['clean'] = True
                t = bytearray(b'\xff' * 32)
                t[24:32] = bytes.fromhex(a.noid_hi)
                j['share_target_hex'] = t.hex()
                j['expires_in_seconds'] = 253
                cc.send_line({'id': None, 'method': 'mining.notify', 'params': [j]})
                ev('job', j['job_id'])
            every(c, job)
        elif m == 'mining.submit':
            ev('share', '%s %s' % (c.login, o['params'][0]))
            c.later({'id': o['id'], 'result': True, 'error': None}, {'id': o['id'], 'result': None, 'error': {'code': -32000, 'message': 'rejected'}})
        elif m == 'mining.hashrate':
            c.send_line({'id': o['id'], 'result': True, 'error': None})


# ---------------------------------------------------------------- prox (suprnova-format stratum)
def prox(c):
    en1 = '%04x' % random.getrandbits(16)
    for o in lines(c):
        m = o.get('method')
        if m == 'mining.subscribe':
            c.send_line({'id': o['id'], 'result': [[['mining.notify', 'x']], en1, 6], 'error': None})
        elif m == 'mining.authorize':
            c.login = '%s' % o['params'][0]
            ev('login', c.login)
            c.send_line({'id': o['id'], 'result': True, 'error': None})
            c.send_line({'id': None, 'method': 'mining.set_difficulty', 'params': [float(2 ** a.k)]})

            def job(cc, n):
                rnd = random.Random(n * 7919 + a.port)
                pre = bytes(rnd.getrandbits(8) for _ in range(369))
                jid = 'x%d_%d' % (a.port, n)
                cc.send_line({'id': None, 'method': 'mining.notify', 'params': [jid, 'txid-blake2b', pre.hex(), len(pre), 200000 + n, int(a.interval * 1000) + 2000, True]})
                ev('job', jid)
            every(c, job)
        elif m == 'mining.submit':
            ev('share', '%s %s' % (c.login, o['params'][1]))
            c.later({'id': o['id'], 'result': True, 'error': None}, {'id': o['id'], 'result': None, 'error': [23, 'rejected', None]})
        elif 'id' in o:  # an unknown method: an error answer
            ev('unknown', str(m))
            c.send_line({'id': o['id'], 'result': None, 'error': [20, 'method not supported', None]})


# ---------------------------------------------------------------- innov
def innov(c):
    for o in lines(c):
        m = o.get('method')
        if m == 'login':
            c.login = '%s' % o['params']['user']
            ev('login', c.login)
            c.send_line({'id': o['id'], 'result': {'proto': 1, 'session': 'mock'}})

            def job(cc, n):
                rnd = random.Random(n * 7919 + a.port)
                msg = bytes(rnd.getrandbits(8) for _ in range(369)).hex()
                jid = 'i%d_%d' % (a.port, n)
                cc.send_line({'method': 'job', 'params': {'job_id': jid, 'message': msg, 'share_k': a.k, 'share_beat': 'ff' * 26, 'k_net': 47, 'ttl_ms': int(a.interval * 1000) + 2000, 'clean': True}})
                ev('job', jid)
            every(c, job)
        elif m == 'submit':
            ev('share', '%s %s' % (c.login, o['params']['job_id']))
            c.later({'id': o['id'], 'result': {'accepted': True}}, {'id': o['id'], 'result': {'accepted': False}})
        elif m == 'keepalive':
            c.send_line({'id': o['id'], 'result': 'ok'})


# ---------------------------------------------------------------- cmfd (the miner's failback check only)
def read_frame(s):
    h = b''
    while len(h) < 4:
        d = s.recv(4 - len(h))
        if not d:
            return None
        h += d
    n = struct.unpack('>I', h)[0]
    b = b''
    while len(b) < n:
        d = s.recv(n - len(b))
        if not d:
            return None
        b += d
    return json.loads(b)


def cmfd(c):
    nid, fp, nonce = [1] * 32, [2] * 32, [3] * 32
    c.send_frame({'type': 'auth_challenge', 'protocol_version': 1, 'network_id': nid, 'consensus_fingerprint': fp, 'nonce': nonce})
    try:
        hello = read_frame(c.s)
    except (OSError, ValueError):
        return
    if not hello or hello.get('type') != 'hello':
        return
    c.login = '%s %s' % (bytes(hello.get('payout', [])).hex(), hello.get('worker'))
    ev('login', c.login)
    c.send_frame({'type': 'hello_ack', 'protocol_version': 1, 'network_id': nid, 'consensus_fingerprint': fp, 'session_id': 1, 'accounting_semantics': 'mock', 'persistence': 'mock'})
    c.send_frame({'type': 'job', 'job': {'mock': True}})
    ev('job', 'cmfd')
    while c.alive:  # stay connected like a pool would
        time.sleep(0.2)
        try:
            c.s.setblocking(False)
            if c.s.recv(65536) == b'':
                break
        except BlockingIOError:
            pass
        except OSError:
            break


HANDLER = dict(pearl=pearl, noid=noid, prox=prox, innov=innov, cmfd=cmfd)[a.mode]
ctx = None
if a.mode == 'cmfd' and not a.tls:
    sys.exit('cmfd mode needs --tls CERT KEY')
if a.tls:
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.minimum_version = ssl.TLSVersion.TLSv1_3 if a.mode == 'cmfd' else ssl.TLSVersion.TLSv1_2
    ctx.load_cert_chain(a.tls[0], a.tls[1])


def serve(s):
    c = Conn(s)
    try:
        if ctx:
            c.s = ctx.wrap_socket(s, server_side=True)
        HANDLER(c)
    except (OSError, ssl.SSLError):
        pass
    finally:
        c.alive = False
        ev('close')
        try:
            c.s.close()
        except OSError:
            pass


srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(('127.0.0.1', a.port))
srv.listen(16)
ev('listening', str(a.port))
if a.mute_after > 0:
    srv.settimeout(0.2)
while True:
    if a.mute_after > 0 and time.time() - START >= a.mute_after:
        srv.close()  # no new connections; the open ones go silent (Conn.muted)
        ev('mute')
        while True:
            time.sleep(3600)
    try:
        cs, _ = srv.accept()
    except socket.timeout:
        continue
    threading.Thread(target=serve, args=(cs,), daemon=True).start()
