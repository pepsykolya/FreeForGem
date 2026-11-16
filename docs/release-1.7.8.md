# FreeForGem 1.7.8 — release notes

[English](#english) · [Русский](#русский)

## English

Files: `freeforgem-1.7.8_hiveos.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.8/freeforgem-1.7.8_hiveos.tar.gz`),
`freeforgem-1.7.8-linux-x86_64.tar.gz`, `freeforgem-1.7.8-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.8_hiveos.tar.gz` | `34ebd83323e99c83a4fa731f26f2eba55ebf15ea1cecd746926171895ff97bf1` |
| `freeforgem-1.7.8-linux-x86_64.tar.gz` | `d8a16d92ed5c3c28b55e38fe4dc5b903259a79f26fc0ccc6c49e9065e53a0525` |
| `freeforgem-1.7.8-windows-x64.zip` | `1df6243caca3b51a86258f7f962f0f61333ee2745f74b6791f8fc733634fe09e` |

**New coin: Proxima (PROX)** — `--algo prox`, NVIDIA GTX 10 / RTX 20/30/40/50.

**Connect (HiveOS custom miner).** Flight sheet: Coin `PROX`, Wallet = your PROX address (InnovLab: holder id), Pool *Configure in miner*,
Miner *Custom* → *Setup Miner Config*:

| Field | proximamine | InnovLab | solo (your node) |
|---|---|---|---|
| Miner name | `freeforgem` | `freeforgem` | `freeforgem` |
| Installation URL | the URL above | same | same |
| Hash algorithm | leave `----` (HiveOS has no `proxima` in its list) | same | same |
| Wallet and worker template | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%` | `%WORKER_NAME%` |
| Pool URL | `stratum+ssl://pool.proximamine.com:3334` | `innov+tls://eu3.innovlab.cc:19701` | `http://NODE_IP:PORT` |
| Pass | your proximamine secret | `x` | the node's `mine.seeker.token` |
| Extra config arguments | `--auto-uv --lock-cclock 1500` | same | same |

Recommended Extra config `--auto-uv --lock-cclock <MHz>` (RTX 3070: `1500`, measured 2.66 GH/s, 87 W, 0.031 GH/W; RTX 3070 Laptop `1500`
+270, 2.23 GH/s, 53 W; RTX 3080 Ti `1500` +210, 4.40 GH/s, 147 W; RTX 4070 Ti `2450` +270, 5.47 GH/s, 142 W, 0.0385 GH/W; CMP 50HX not
measured yet). It can simply be deleted. Windows: `start-prox.bat` (`WALLET`, `POOL`, `PASSWORD`, optional `AUTO_UV=1`, `LOCK_CCLOCK=1500`).
Details: MANUAL.md, section 21.

* Own closed-source BLAKE2b kernel (the one of our ffgprox miner); every candidate re-hashed on the CPU before it is sent.
* Pools: proximamine (suprnova-format stratum, TLS), InnovLab (native protocol), any suprnova-format pool; solo through `proxi node mine`
  (seeker protocol v2). The miner identifies itself as `FreeForGem/1.7.8` (suprnova only admits its own miner's user agent).
* `--auto-uv` for PROX (optional): every step checked bit-exactly in a child process, profile per GPU / driver / lock, re-check every 6 h.
* The memory clock is locked at the lowest value that keeps the full core range by default (`--no-mem-lock` to turn it off).
* HiveOS: GH/s, GH/W next to the version, accepted / rejected, `need update!!` as for the other coins.
* Dev fee 1% (60 s per 100 min) on the pool you mine on; the developer's proximamine account in solo mode, on suprnova, or when the fee
  login on your pool fails.

**Backup pools: failover and failback (all coins).** `--pool A,B` (or `--pool` repeated; HiveOS: several URLs in *Pool URL*, Windows:
`POOL=A,B`) is a list of one coin: the first entry is the primary. A pool that is unreachable, refuses the login or sends no job moves the
miner to the next entry; while on a backup the primary is checked every `--failback-minutes` (default 10, 0 = off) on a separate
connection (connect + login + a job), and after 2 good checks in a row the miner goes back, keeping the shares in flight (the switch
waits 1-3 s for the old pool's answers; the GPUs keep their job). The dev fee connection always goes to the pool you are on.
Before 1.7.8 FreeForGem had no backup pool at all (one `--pool`, endless reconnects). Same names and defaults as ffgs 0.5.4. CMFD:
the engine has one pool, so the miner restarts the engines on the next entry (about a minute without mining per switch).
`--pool-check` tests every entry without a GPU. MANUAL.md section 22.

**Dead pool link found in about 60 s.** A pool that goes silent without closing the connection (a pulled cable, a lost route, a firewall DROP) used to be noticed only after 180-300 s of idle GPUs; now in about 60 s, then reconnect and, if the entry is gone, failover. InnovLab (PROX) answers the miner's keepalive (20 s), NOID the hashrate report (30 s), the Pearl and PROX stratum a `mining.ping` sent after 20 s of silence and once 20 s after connecting (their answer is an error, which counts); once a pool answered a probe, 60 s without any byte end the connection. A pool that never answers probes keeps the old limit (180 / 240 / 300 s), so a pool that is quiet between jobs is never cut off by mistake. CMFD: the engine owns its pool connection (unchanged). Test: `tests/silence_test.sh` (a mock pool that goes half-open: failover within 75 s; a pool with a job every 45 s: no reconnect).

**CMFD silent connection and the shares watchdog.** The engine keeps its connection itself and did not notice a link that died without a reset (DROP both ways) for 6+ minutes while the block height stood. The miner now watches the height the engines report: when it has not changed for 12 mean block intervals (measured on the run; never less than 8 nor more than 30 minutes) the engines restart on the next `--pool` entry. The watchdog's expected share rate is now kept per pool and started over on every pool change (an easy pool's rate used to cause a false "no share found ... although ~10 were expected" after returning to a hard one). `--auto-lock` on CMP cards: the default top is 1500 MHz (was 2000, which only ran into the power limit); a step at the power limit is marked PL-limited and the sweep goes on at once. CMP 50HX on driver 610.43.03: Pearl ~67 TH/s at lock 1300 and ~77 TH/s at 1500 (166-176 W). MANUAL.md, sections 22 and 23.

**Pool that credits nothing (accepted-shares watchdog).** While the GPUs hash for you, if there is no accepted share of yours for
`--noshare-minutes` (default 15, 0 = off) with at least 10 shares expected at your hash rate and the pool's share difficulty, or only
rejected shares for `--reject-minutes` (default 5; at least 5 rejects), the miner reconnects; the same again on the new connection and it
moves to the next `--pool` entry (a primary that was left this way is checked less often, x2 up to x16); a single pool is reconnected
with a warning in the log, the miner never exits. It does not count the dev fee minute, a GPU that found nothing (warning only) or rare
big-difficulty shares (CMFD: no difficulty is known, shares are expected at the rate seen so far, one per 6 min until there are 3).
Same names and defaults as ffgs 0.5.4.
**New: `--auto-lock`** (optional, off by default; Pearl, NOID, PROX; NVIDIA) — the core clock lock of each GPU chosen by measurement:
the locks of a range (default 1800 ... 1200 MHz) are mined in turn, each at its own auto-uv profile, and the most efficient lock within 10 %
of the top hash rate is kept and saved (`autolock.json`); later starts apply it at once. Ported from ffgs 0.5.4 (same options:
`--auto-lock-range`, `--auto-lock-max-loss`, `--auto-lock-goal`, `--auto-lock-reset`). HiveOS: add `--auto-lock` to Extra config arguments.
Not for CMFD. Details: MANUAL.md, section 23.
**`--auto-lock-goal profit`** — the lock with the most US$ per day: income (hash rate x coins per H/s from the network: Pearl and NOID
from the pool's jobs, PROX from InnovLab's stats; x the coin price x 0.99 x (1 - pool fee)) minus electricity (`--power-price`, required;
day / night `0.12,0.053@23-07`). The coin price is the median of public tickers (Pearl: 9, NOID: 3, PROX: SafeTrade only; outliers
> 15 % and thin tickers left out; GET only, nothing of yours is sent) or `--coin-price X` (no exchange asked). Every lock of 2000 ... 1200
is measured; every `--auto-lock-reprice-hours` (6) the lock is re-chosen from the measured points, a switch needs > 3 %
(`--auto-lock-hysteresis`). PROX efficiency in the auto-lock tables: GH/W. MANUAL.md, section 23.1 (also: privacy).

**Auto-lock details.** A step at the power limit (90 %+ of it, the clock under the lock for 20 s of mining) is now decided in the first half minute, before any offset search (it was found only after the full auto-uv, ~9 min per step on an RTX 3080 Ti). GeForce RTX 30 / RTX A start at 1800 MHz (CMP: 1500). `--minutes N` now also stops a running sweep cleanly (clocks and offsets put back) and counts from the start of the process. Windows: OS error texts are in English instead of unreadable characters in a console of another code page (`FFG_OS_LANG=system` keeps the system language).

**Other 1.7.8 changes**

* **auto-uv, Pearl: more checks at once on a rig with plenty of RAM.** The check plan now takes the free RAM minus a 512 MB margin instead
  of 70 % of it; on a low-RAM rig (the proofs of the checks are serialized) the 70 % share stays. The check processes prove one at a
  time (a check-proof lock, one shared `A` in the RAM plan), so an 8-GPU rig runs all 8 checks together (was 3; minimum available RAM
  6.5 GB on the 8x RTX 3070 rig) and tunes from scratch in about 11 minutes. The power of a check is measured over its load only, not over
  its last CPU verification: a warming card is no longer reported as having reached the voltage floor.
* **CMFD engine 1.2.5 (compact-mainnet fingerprint).** The pool client accepts both consensus fingerprints (the old one and the
  compact-mainnet one that comes with the fork at height 67 535, same network ID), so the miner keeps working when the pools upgrade. An
  unknown fingerprint is still refused. The Linux engine is still built for Ubuntu 20.04 (glibc 2.30+), so it runs on every HiveOS.
* **PROX integration:** PROX and NOID no longer need `pearlgpu.dll` just to find the PCI bus ids of their cards for the clock code
  (Windows: a folder with only `freeforgem.exe` and `proxgpu.dll` works). HiveOS has no `proxima` in the *Hash algorithm* list: leave
  `----`. PROX writes only the `prox` section of `uv.json`. On exit only the locks this run set are put back (the HiveOS OC profile lock,
  else a reset).
* **HiveOS OC applies no longer undo the clocks (all coins).** `nvidia-oc` resets the clock locks and the offset at every miner start and OC
  change. The miner now notices a core / memory lock that is lost under load (a core clock steadily above the lock, a memory clock off
  it) and sets it again, one log line per event, at most 4 times per GPU per hour. PROX gets what Pearl and NOID already had for the
  offset: the verified `--auto-uv` offset (after a 20 s check of that GPU) and a fixed `--uv-offset` are set again after an OC apply.
* **auto-uv, all coins (from the 1.7.7 plan):** a CUDA fault on one card ends the whole process; at the next start only the card that had
  the Xid counts its offset under check as failed, the other cards that were checking get the check repeated and keep the steps that had
  passed. A start offset that fails its check is checked once more before the search goes lower (one failure can be a transient one).

## Русский

Файлы: `freeforgem-1.7.8_hiveos.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.8/freeforgem-1.7.8_hiveos.tar.gz`),
`freeforgem-1.7.8-linux-x86_64.tar.gz`, `freeforgem-1.7.8-windows-x64.zip`. Проверьте SHA-256 (MANUAL.md, раздел 9).

**Новая монета: Proxima (PROX)** — `--algo prox`, NVIDIA GTX 10 / RTX 20/30/40/50.

**Подключение (HiveOS custom miner).** Полётный лист: монета `PROX`, кошелёк — ваш адрес PROX (InnovLab — holder id), пул *Configure in
miner*, майнер *Custom* → *Setup Miner Config*:

| Поле | proximamine | InnovLab | соло (своя нода) |
|---|---|---|---|
| Miner name | `freeforgem` | `freeforgem` | `freeforgem` |
| Installation URL | ссылка выше | то же | то же |
| Hash algorithm | оставить `----` (в списке HiveOS нет `proxima`) | так же | так же |
| Wallet and worker template | `%WAL%.%WORKER_NAME%` | `%WAL%.%WORKER_NAME%` | `%WORKER_NAME%` |
| Pool URL | `stratum+ssl://pool.proximamine.com:3334` | `innov+tls://eu3.innovlab.cc:19701` | `http://IP_НОДЫ:ПОРТ` |
| Pass | secret аккаунта proximamine | `x` | `mine.seeker.token` ноды |
| Extra config arguments | `--auto-uv --lock-cclock 1500` | то же | то же |

Рекомендуемый Extra config `--auto-uv --lock-cclock <МГц>` (RTX 3070: `1500`, замер 2,66 GH/s, 87 Вт, 0,031 GH/W; RTX 4070 Ti: `2450`,
+270, 5,47 GH/s, 142 Вт, 0,0385 GH/W; остальные карты пока не замерены). Строку можно просто удалить. Windows: `start-prox.bat` (`WALLET`, `POOL`, `PASSWORD`, по желанию `AUTO_UV=1`, `LOCK_CCLOCK=1500`).
Подробно: MANUAL.md, раздел 21.

* Своё закрытое ядро BLAKE2b (из нашего майнера ffgprox); каждый кандидат перед отправкой пересчитывается на CPU.
* Пулы: proximamine (stratum формата suprnova, TLS), InnovLab (родной протокол), любой пул формата suprnova; соло через `proxi node mine`
  (seeker v2). Майнер представляется как `FreeForGem/1.7.8` (suprnova пускает только user agent своего майнера).
* `--auto-uv` для PROX (по желанию): каждый шаг проверяется бит в бит в отдельном процессе, профиль по GPU / драйверу / локу, перепроверка раз в 6 ч.
* Память по умолчанию лочится на самой низкой частоте, при которой ядру доступен весь диапазон (`--no-mem-lock` — выключить).
* HiveOS: GH/s, GH/W рядом с версией, принятые / отклонённые, `need update!!` — как у других монет.
* Комиссия 1 % (60 с раз в 100 мин) на том пуле, где вы майните; аккаунт разработчика на proximamine — в соло, на suprnova и если fee-логин
  на вашем пуле не прошёл.

**Резервные пулы: failover и failback (все монеты).** `--pool A,B` (или несколько `--pool`; HiveOS: несколько URL в *Pool URL*, Windows:
`POOL=A,B`) — список одной монеты, первая запись основная. Недоступный пул, отказ в логине или отсутствие задания переводят майнер на
следующую запись; пока майнер на резервном, основной проверяется раз в `--failback-minutes` (по умолчанию 10, 0 — выкл.) отдельным
соединением (подключение + login + задание), и после 2 удачных проверок подряд майнер возвращается, сохраняя шары «в полёте» (переключение
ждёт 1-3 с ответов старого пула; GPU не теряют задание). Соединение комиссии всегда идёт на тот пул, где вы сейчас.
До 1.7.8 резервных пулов у FreeForGem не было вовсе (один `--pool`, бесконечные переподключения). Имена и значения как в ffgs 0.5.4.
CMFD: у движка один пул, поэтому майнер перезапускает движки на следующей записи (около минуты без майнинга на каждое переключение).
`--pool-check` проверяет каждую запись без GPU. MANUAL.md, раздел 22.

**Мёртвая связь с пулом обнаруживается за ~60 с.** Пул, замолчавший без закрытия соединения (выдернутый кабель, пропавший маршрут, DROP в файрволе), раньше замечали только через 180-300 с простоя GPU; теперь примерно за 60 с, затем переподключение и, если записи нет, переход на резервный. InnovLab (PROX) отвечает на keepalive майнера (20 с), NOID — на отчёт о хешрейте (30 с), stratum Pearl и PROX — на `mining.ping` после 20 с тишины и один раз через 20 с после подключения (ответ — ошибка, это считается); если пул хоть раз ответил на проверку, 60 с без единого байта обрывают соединение. Пул, не отвечающий на проверки, сохраняет прежний предел (180 / 240 / 300 с), поэтому молчащий между заданиями пул никогда не отключается по ошибке. CMFD: соединение с пулом держит движок (без изменений). Тест: `tests/silence_test.sh` (мок-пул, который замолкает: переход за 75 с; пул с заданием раз в 45 с: без переподключений).

**CMFD: молчащее соединение и сторож шар.** Движок держит соединение сам и обрыв без reset (DROP в обе стороны) не замечал 6+ минут при стоящей высоте блока. Теперь майнер следит за высотой, которую сообщают движки: если она не меняется дольше 12 средних интервалов между блоками (измеряются в запуске; не меньше 8 и не больше 30 минут), движки перезапускаются на следующей записи `--pool`. Ожидаемая частота шар сторожа теперь считается по каждому пулу отдельно и сбрасывается при смене пула (раньше частота лёгкого пула давала ложное «no share found ... although ~10 were expected» после возврата на тяжёлый). `--auto-lock` на CMP: верх диапазона по умолчанию 1500 МГц (был 2000, упирался в лимит мощности), шаг у лимита мощности помечается PL-limited и перебор идёт дальше сразу. CMP 50HX на драйвере 610.43.03: Pearl ~67 TH/s при локе 1300 и ~77 TH/s при 1500 (166-176 Вт). MANUAL.md, разделы 22 и 23.

**Пул, который ничего не засчитывает (watchdog принятых шар).** Пока GPU хешируют на вас: нет принятой шары `--noshare-minutes` (по
умолчанию 15, 0 — выкл.) при ≥10 ожидаемых по вашему хешрейту и сложности пула шарах, либо одни отказы `--reject-minutes` (5; не меньше
5 отказов) — майнер переподключается; повторилось на новом соединении — переходит на следующую запись `--pool` (покинутый так основной
проверяется реже, ×2…×16); одиночный пул переподключается с предупреждением в логе, майнер не завершается. Не учитывается минута
комиссии, GPU, не нашедшие ничего (только предупреждение), и редкие шары большой сложности (CMFD: сложность неизвестна, шары ожидаются
по уже виденной частоте, по одной в 6 мин, пока их нет трёх). Имена и значения как в ffgs 0.5.4.
**Новое: `--auto-lock`** (по желанию, по умолчанию выключено; Pearl, NOID, PROX; NVIDIA) — лок частоты ядра каждой карты выбирается
замером: локи диапазона (по умолчанию 1800 ... 1200 МГц) по очереди майнятся, каждый со своим профилем auto-uv, и самый экономичный лок в
пределах 10 % от максимального хешрейта сохраняется (`autolock.json`); следующие старты сразу ставят его. Перенесено из ffgs 0.5.4 (те же
опции: `--auto-lock-range`, `--auto-lock-max-loss`, `--auto-lock-goal`, `--auto-lock-reset`). HiveOS: `--auto-lock` в Extra config
arguments. Не для CMFD. Подробно: MANUAL.md, раздел 23.
**`--auto-lock-goal profit`** — лок с наибольшей прибылью в $ в сутки: доход (хешрейт x монет на H/s по данным сети: Pearl и NOID — из
заданий пула, PROX — из статистики InnovLab; x цена монеты x 0,99 x (1 - комиссия пула)) минус электричество (`--power-price`,
обязателен; день / ночь `0.12,0.053@23-07`). Цена монеты — медиана публичных тикеров (Pearl: 9, NOID: 3, PROX: только SafeTrade; выбросы
> 15 % и тонкие тикеры отбрасываются; только GET, ничего вашего не отправляется) или `--coin-price X` (к биржам не обращается). Меряются
все локи 2000 ... 1200; раз в `--auto-lock-reprice-hours` (6) лок перевыбирается по замеренным точкам, переключение — при выигрыше > 3 %
(`--auto-lock-hysteresis`). Эффективность PROX в таблицах auto-lock — GH/W. MANUAL.md, раздел 23.1 (там же — конфиденциальность).

**Auto-lock, детали.** Шаг в лимите мощности (90 %+ от него, частота ниже лока 20 с работы) теперь определяется в первые полминуты, до подбора оффсета (раньше — только после полного auto-uv, ~9 мин на шаг на RTX 3080 Ti). GeForce RTX 30 / RTX A начинают с 1800 МГц (CMP: 1500). `--minutes N` теперь останавливает и идущий свип (чисто, локи и оффсеты возвращаются) и считается от старта процесса. Windows: тексты ошибок ОС по-английски, а не нечитаемыми символами в консоли другой кодировки (`FFG_OS_LANG=system` оставляет язык системы).

**Остальные изменения 1.7.8**

* **auto-uv, Pearl: больше проверок одновременно на риге с запасом RAM.** План проверок берёт свободную RAM минус запас 512 МБ вместо 70 % от
  неё; на риге с малой RAM (доказательства проверок идут по одному) остаётся доля 70 %. Процессы проверок доказывают по одному (замок
  check-proof, один общий `A` в плане RAM), поэтому риг на 8 GPU запускает все 8 проверок сразу (было 3; минимум доступной RAM 6,5 ГБ на
  риге 8x RTX 3070) и тюнится с нуля примерно за 11 минут. Мощность проверки меряется только на её нагрузке, а не на последней CPU-сверке:
  прогревающаяся карта больше не считается упёршейся в voltage floor.
* **Движок CMFD 1.2.5 (compact-mainnet fingerprint).** Пул-клиент принимает оба consensus fingerprint (старый и compact-mainnet, который
  приходит с форком на высоте 67 535, network ID тот же), поэтому майнер продолжит работать, когда пулы обновятся. Неизвестный fingerprint
  по-прежнему отклоняется. Linux-движок по-прежнему собирается под Ubuntu 20.04 (glibc 2.30+) и работает на любом HiveOS.
* **Интеграция PROX:** PROX и NOID больше не требуют `pearlgpu.dll` только ради PCI bus id своих карт для кода частот (Windows: работает папка
  только с `freeforgem.exe` и `proxgpu.dll`). В списке *Hash algorithm* HiveOS нет `proxima`: оставьте `----`. PROX пишет только раздел `prox`
  в `uv.json`. При выходе возвращаются только те локи, что поставил этот запуск (лок из профиля HiveOS, иначе сброс).
* **Применение разгона HiveOS больше не отменяет частоты (все монеты).** `nvidia-oc` сбрасывает локи частот и оффсет при каждом старте майнера и
  смене разгона. Теперь майнер замечает потерянный под нагрузкой лок ядра / памяти (частота ядра устойчиво выше лока, частота памяти мимо
  лока) и ставит его заново, одна строка в логе на событие, не больше 4 раз на GPU в час. PROX получил то, что у Pearl и NOID уже было для
  оффсета: проверенный оффсет `--auto-uv` (после 20-секундной проверки этой GPU) и фиксированный `--uv-offset` ставятся заново после
  применения разгона.
* **auto-uv, все монеты (из плана 1.7.7):** сбой CUDA на одной карте останавливает весь процесс; при следующем старте проверяемый оффсет
  засчитывается неудачным только карте с Xid, остальные проверявшиеся карты повторяют проверку и сохраняют уже пройденные шаги. Стартовый
  оффсет, не прошедший проверку, перепроверяется один раз, прежде чем поиск пойдёт вниз (один сбой может быть случайным).
