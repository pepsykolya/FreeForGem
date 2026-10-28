# FreeForGem 1.7.3 — release notes

[English](#english) · [Русский](#русский)

## English

Files: `freeforgem-1.7.3.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.3/freeforgem-1.7.3.tar.gz`),
`freeforgem-1.7.3-linux-x86_64.tar.gz`, `freeforgem-1.7.3-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.3.tar.gz` | `6c29130fc6ba4660ac6b1d90dccd3667e9bcb112fc1441bbfd3c2a0e4037d126` |
| `freeforgem-1.7.3-linux-x86_64.tar.gz` | `59f94d03364f8908340e963190927c5e2399f2de2045a4c519905f89b4a51325` |
| `freeforgem-1.7.3-windows-x64.zip` | `6a53f9338834be12213493a53df37f32c34682712f73f868005611484e89ca53` |

Checked on HiveOS rigs, Pearl, core lock 1805, mem 810, `--auto-uv`: 8x RTX 3070 — 647.8 TH/s, 1179 W, 0.550 TH/W
(about an hour, no rejected shares, peak RAM 4.3 GB of 12 GB); 4x RTX 3070 — 326.2 TH/s, 547 W, 0.596 TH/W.

**Fixed: Pearl on rigs with little RAM** (a 4 GB rig with 5x CMP 50HX swapped for minutes, then the OOM killer stopped the miner):

* **Far less RAM for Pearl.** A job kept its 0.7-0.8 GB matrix A in memory, and every proof copied it on the GPU's own prover
  thread. Now A is built only while a proof needs it. RTX 3070: the miner 925 → 205 MB (median), a check process 2378 → 1666 MB,
  a job is ready in 60-75 ms instead of 0.5 s.
* **Proofs share the RAM:** more at once only while half of the free RAM covers another (at most 4); on a rig low on RAM one proof
  at a time on the whole machine, the miner's and its check processes'.
* **`--auto-uv` plans its checks with the RAM:** 70 % of the free RAM (also a cgroup limit) minus what mining needs; the log says
  `checking N of M GPU(s) at a time`. When not even one fits, no checks or tunings run: a GPU with a verified profile uses it,
  the others mine at your offset. 1.7.2 started the first check whatever the RAM.
* **The OOM killer takes a check process before the miner** (oom_score_adj); a check killed for lack of RAM is not counted as a
  failed offset, and no more checks run that time.
* **HiveOS locks:** without `--lock-cclock` / `--lock-mclock` the OC profile's locks are set before mining (another program may
  have left the clocks unlocked). "Memory lock not supported" on CMP cards is a calm note now.
* **The GPUs start mining one by one**, 1.5 s apart.

Test (RTX 3070, 1 GPU, a memory cap like a 4 GB 5-GPU rig: 2.9 GB): 1.7.2 sat at the cap and did not mine; 1.7.3 tuned
(+240), mined, peak 1.65 GB, no OOM, 11 of 11 shares accepted.

**New: NACRE (NACR) merge mining on HimPool and AlphaPool.** `--wallet prl1...+nacr1p...` (HimPool also
`+NOCK_ADDRESS` after it): the miner keeps the `+` parts on these pools and appends the worker after them. 1.7.2 dropped
everything after `+` outside LuckyPool, so these pools got PRL only. Other pools: unchanged.

**Fixed: AlphaPool.** The pool closes the connection on `mining.subscribe`; 1.7.3 logs in with `mining.authorize` only
there (1.7.2 kept reconnecting and got no jobs). Test, RTX 3070, 5 min: 107 of 108 shares accepted.

## Русский

Файлы: `freeforgem-1.7.3.tar.gz` (ссылка для установки HiveOS
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.3/freeforgem-1.7.3.tar.gz`),
`freeforgem-1.7.3-linux-x86_64.tar.gz`, `freeforgem-1.7.3-windows-x64.zip`. Сверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.7.3.tar.gz` | `6c29130fc6ba4660ac6b1d90dccd3667e9bcb112fc1441bbfd3c2a0e4037d126` |
| `freeforgem-1.7.3-linux-x86_64.tar.gz` | `59f94d03364f8908340e963190927c5e2399f2de2045a4c519905f89b4a51325` |
| `freeforgem-1.7.3-windows-x64.zip` | `6a53f9338834be12213493a53df37f32c34682712f73f868005611484e89ca53` |

Проверено на ригах HiveOS, Pearl, лок ядра 1805, память 810, `--auto-uv`: 8x RTX 3070 — 647,8 TH/s, 1179 Вт, 0,550 TH/W
(около часа, без отклонённых шар, пик памяти 4,3 ГБ из 12 ГБ); 4x RTX 3070 — 326,2 TH/s, 547 Вт, 0,596 TH/W.

**Исправлено: Pearl на ригах с малым объёмом ОЗУ** (риг с 4 ГБ и 5x CMP 50HX минутами сидел в свопе, затем OOM killer остановил
майнер):

* **Pearl нужно гораздо меньше памяти.** Задание держало в памяти матрицу A на 0,7-0,8 ГБ, а каждое доказательство копировало её
  в потоке своей карты. Теперь A строится только на время доказательства. RTX 3070: майнер 925 → 205 МБ (медиана), процесс
  проверки 2378 → 1666 МБ, задание готово за 60-75 мс вместо 0,5 с.
* **Доказательства делят память:** несколько сразу — только пока половина свободной памяти покрывает ещё одно (не больше 4); при
  нехватке памяти — одно за раз на всю машину, у майнера и его процессов проверки.
* **`--auto-uv` планирует проверки по памяти:** 70 % свободной памяти (и лимита cgroup) минус нужное майнингу; в логе
  `checking N of M GPU(s) at a time`. Если не помещается даже одна — проверок и тюнинга нет: карта с проверенным профилем
  использует его, остальные майнят на вашем offset. 1.7.2 запускал первую проверку при любой памяти.
* **OOM killer убивает процесс проверки раньше майнера** (oom_score_adj); проверка, убитая из-за нехватки памяти, не считается
  провалом offset, и проверок в этом запуске больше нет.
* **Локи HiveOS:** без `--lock-cclock` / `--lock-mclock` локи профиля OC ставятся до начала майнинга (другая программа могла
  оставить частоты без лока). «Memory lock not supported» на картах CMP — теперь спокойная заметка.
* **Карты начинают майнить по очереди**, через 1,5 с.

Тест (RTX 3070, 1 карта, лимит памяти как у рига 4 ГБ с 5 картами: 2,9 ГБ): 1.7.2 упёрся в лимит и не майнил; 1.7.3 подобрал
(+240), майнил, пик 1,65 ГБ, без OOM, 11 из 11 шар принято.

**Новое: merge-майнинг NACRE (NACR) на HimPool и AlphaPool.** `--wallet prl1...+nacr1p...` (на HimPool после него можно
ещё `+АДРЕС_NOCK`): на этих пулах майнер оставляет части `+` и дописывает воркер после них. 1.7.2 вне LuckyPool отбрасывал
всё после `+`, и эти пулы получали только PRL. Остальные пулы — без изменений.

**Исправлено: AlphaPool.** Пул закрывает соединение на `mining.subscribe`; 1.7.3 входит туда только через
`mining.authorize` (1.7.2 переподключался и не получал заданий). Тест, RTX 3070, 5 мин: принято 107 из 108 шар.
