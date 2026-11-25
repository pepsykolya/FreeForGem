# FreeForGem 1.8.1 — release notes

[English](#english) · [Русский](#русский) · [中文](#中文)

## English

Files: `freeforgem-1.8.1_hiveos.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.1/freeforgem-1.8.1_hiveos.tar.gz`),
`freeforgem-1.8.1-linux-x86_64.tar.gz`, `freeforgem-1.8.1-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.8.1_hiveos.tar.gz` | `2fb5d71837319684e7659276dd6bee9345cb05700a9ba41991e541623ff64f0d` |
| `freeforgem-1.8.1-linux-x86_64.tar.gz` | `cea96d59b235259b0e3ddbd03f7ab09710add3e116b01cab883787b5cb5fdaef` |
| `freeforgem-1.8.1-windows-x64.zip` | `495821d38f0485e15016157eb2142a779645d94a38206c41fa9ff45042a6423d` |

Update: paste the new Installation URL into your flight sheet (HiveOS caches a package by its file name). Saved `--auto-uv` results
are kept. Dev fee unchanged: 1 %.

**auto-uv while mining, every coin** (MANUAL.md, section 30)
* The card keeps mining while `--auto-uv` tunes it, on Pearl, NOID, PROX, Ting, CMFD and NEB; shares go to the pool through the whole tuning.
* All cards are tuned at once, as many as the rig's free RAM allows; 1 at a time only after trouble of the whole rig (`--uv-parallel N`,
  `--uv-parallel-reset`).
* A short start (about +90, Turing +60) and no built-in offset ceiling: the climb goes on while each step lowers the power, up to the
  driver's range or your `--uv-max`.
* 4x RTX 3070 from scratch: NEB 1.5-2 min, PROX ~5 (1.8.0: ~16), Ting 6-8.5, NOID ~9 (1.8.0: ~25), CMFD 10-18. Per watt: Ting +15…+17 %,
  CMFD +17…+20 %, NOID +5…+9 %, PROX +2…+8 %; Pearl 8x RTX 3070: 0.660 → 0.677 TH/W at the same hashrate.
* After a failure "failed − 2 steps", never from zero; progress saved after every step; faults counted per coin, a card left out shows
  `OUT 14m` and comes back by itself.
* Turing (RTX 20xx, GTX 16xx, CMP HX): a result is confirmed 5 minutes on its mining load; a second crash within 30 minutes goes at once
  to the stable offset; an older Turing profile is held 5 minutes once (`turing-held.json`).
* The 24-hour guard counts only failures at or below the confirmed offset; one-shot flags (`--uv-reset`, `--uv-reset-all`,
  `--uv-unpause`, `--auto-lock-reset`) act once, at a start you make — do not keep them in the flight sheet.

**The screen: a table of the GPUs** (section 31)
* Hashrate, temperature, fan %, watts, power limit, per watt, clocks / locks, offset and tuning state, shares, BAD, status; the rig total.
* Header: pool and worker, `net` (block, job id and age, share and network difficulty, ping, stale), `mode` (algorithm, locks), `args`
  (secrets masked), up to 5 latest events, live alarms. Orange = the tuning at work, red = needs your hands.
* `--ui table|lines|auto|ascii|unicode`, `--ui-demo`, `--no-color`. The log file and HiveOS h-stats are unchanged.

**HiveOS**
* `--hive-oc-sync`: the tuned offsets and clock locks as a message in the HiveOS panel; with an API token (best from a file with
  `chmod 600`: `--hive-oc-sync-token-file`) written into the worker's OC profile, each `--split` group its own cards (section 32).
* The miner window stays open: a restart loop in h-run.sh, a 10-minute pause after frequent exits.
* `--split` works from a flight sheet of any coin (the flight sheet's pool and wallet go to its coin's group).
* A package name needs exactly one dash (`freeforgem-1.8.1_hiveos.tar.gz`).

**Pools and coins** (section 33)
* A spoiled pool session (a share difficulty dropped by the pool, or late answers with many stale) is replaced: `pool: reconnecting`.
* NEB: engine nebgpu 0.8.4 (faster on Turing); no false `NO TABLE` for a model without a digest table (it needs the full memory clock,
  `--table-mclock 810` leaves it full there).
* `--api-port` on every coin. CMFD efficiency in FW/W.

## Русский

Файлы: `freeforgem-1.8.1_hiveos.tar.gz` (ссылка для HiveOS
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.1/freeforgem-1.8.1_hiveos.tar.gz`),
`freeforgem-1.8.1-linux-x86_64.tar.gz`, `freeforgem-1.8.1-windows-x64.zip`. Проверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.8.1_hiveos.tar.gz` | `2fb5d71837319684e7659276dd6bee9345cb05700a9ba41991e541623ff64f0d` |
| `freeforgem-1.8.1-linux-x86_64.tar.gz` | `cea96d59b235259b0e3ddbd03f7ab09710add3e116b01cab883787b5cb5fdaef` |
| `freeforgem-1.8.1-windows-x64.zip` | `495821d38f0485e15016157eb2142a779645d94a38206c41fa9ff45042a6423d` |

Обновление: вставьте новую ссылку в полётный лист (HiveOS кэширует пакет по имени файла). Сохранённые результаты `--auto-uv`
остаются. Комиссия без изменений: 1 %.

**auto-uv во время майнинга, все монеты** (MANUAL.md, раздел 30)
* Карта продолжает майнить, пока `--auto-uv` её подбирает, на Pearl, NOID, PROX, Ting, CMFD и NEB; шары идут в пул весь подбор.
* Все карты подбираются сразу, сколько позволяет свободная память рига; по одной — только после неприятностей всего рига
  (`--uv-parallel N`, `--uv-parallel-reset`).
* Короткий старт (около +90, Turing +60) и без встроенного потолка смещения: подъём идёт, пока каждый шаг снижает мощность, до
  диапазона драйвера или вашего `--uv-max`.
* 4x RTX 3070 с нуля: NEB 1,5–2 мин, PROX ~5 (1.8.0: ~16), Ting 6–8,5, NOID ~9 (1.8.0: ~25), CMFD 10–18. На ватт: Ting +15…+17 %,
  CMFD +17…+20 %, NOID +5…+9 %, PROX +2…+8 %; Pearl 8x RTX 3070: 0,660 → 0,677 TH/W при том же хешрейте.
* После сбоя «сбойный − 2 шага», никогда с нуля; прогресс сохраняется после каждого шага; сбои считаются по монете, исключённая
  карта показывает `OUT 14m` и возвращается сама.
* Turing (RTX 20xx, GTX 16xx, CMP HX): результат подтверждается 5 минут на нагрузке майнинга; второе падение за 30 минут — сразу на
  стабильное смещение; старый профиль Turing один раз удерживается 5 минут (`turing-held.json`).
* Защита «24 часа» считает только сбои на подтверждённом смещении или ниже; разовые флаги (`--uv-reset`, `--uv-reset-all`,
  `--uv-unpause`, `--auto-lock-reset`) действуют один раз, при вашем старте — не держите их в листе.

**Экран: таблица карт** (раздел 31)
* Хешрейт, температура, вентилятор %, ватты, лимит мощности, на ватт, частоты / локи, смещение и состояние подбора, шары, BAD,
  статус; итог рига.
* Шапка: пул и воркер, `net` (блок, id и возраст задания, сложность шары и сети, пинг, stale), `mode` (алгоритм, локи), `args`
  (секреты скрыты), до 5 последних событий, живые тревоги. Оранжевое — подбор работает, красное — нужны ваши руки.
* `--ui table|lines|auto|ascii|unicode`, `--ui-demo`, `--no-color`. Лог-файл и h-stats HiveOS без изменений.

**HiveOS**
* `--hive-oc-sync`: подобранные смещения и локи — сообщением в панель HiveOS; с API-токеном (лучше из файла с `chmod 600`:
  `--hive-oc-sync-token-file`) — запись в профиль разгона воркера, каждая группа `--split` — свои карты (раздел 32).
* Окно майнера не закрывается: цикл перезапуска в h-run.sh, пауза 10 минут после частых выходов.
* `--split` работает с листом любой монеты (пул и кошелёк листа идут в группу его монеты).
* В имени пакета ровно один дефис (`freeforgem-1.8.1_hiveos.tar.gz`).

**Пулы и монеты** (раздел 33)
* Испорченная сессия пула (пул уронил сложность шары или отвечает поздно при большом stale) заменяется: `pool: reconnecting`.
* NEB: движок nebgpu 0.8.4 (быстрее на Turing); нет ложной `NO TABLE` для модели без таблицы (ей нужна полная частота памяти,
  `--table-mclock 810` там её не опускает).
* `--api-port` на всех монетах. Эффективность CMFD — в FW/W.

## 中文

文件：`freeforgem-1.8.1_hiveos.tar.gz`（HiveOS 安装链接
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.1/freeforgem-1.8.1_hiveos.tar.gz`），
`freeforgem-1.8.1-linux-x86_64.tar.gz`、`freeforgem-1.8.1-windows-x64.zip`。请校验 SHA-256（MANUAL.md 第 9 节）。

| 文件 | SHA-256 |
|---|---|
| `freeforgem-1.8.1_hiveos.tar.gz` | `2fb5d71837319684e7659276dd6bee9345cb05700a9ba41991e541623ff64f0d` |
| `freeforgem-1.8.1-linux-x86_64.tar.gz` | `cea96d59b235259b0e3ddbd03f7ab09710add3e116b01cab883787b5cb5fdaef` |
| `freeforgem-1.8.1-windows-x64.zip` | `495821d38f0485e15016157eb2142a779645d94a38206c41fa9ff45042a6423d` |

升级：把新的安装链接填入飞行表（HiveOS 按文件名缓存软件包）。已保存的 `--auto-uv` 结果会保留。开发者抽水不变：1 %。

**挖矿期间的 auto-uv，所有币种**（MANUAL.md 第 30 节）
* `--auto-uv` 调校时显卡继续挖矿，适用于 Pearl、NOID、PROX、Ting、CMFD 和 NEB；整个调校期间份额都提交到矿池。
* 所有显卡同时调校，数量以矿机空闲内存为限；只有整台矿机出问题后才一次一块（`--uv-parallel N`、`--uv-parallel-reset`）。
* 起点更高（约 +90，Turing +60），没有内置偏移上限：只要每一步都降低功耗就继续爬升，直到驱动范围或你的 `--uv-max`。
* 4x RTX 3070 从头调校：NEB 1.5–2 分钟，PROX 约 5（1.8.0：约 16），Ting 6–8.5，NOID 约 9（1.8.0：约 25），CMFD 10–18。每瓦：
  Ting +15…+17 %，CMFD +17…+20 %，NOID +5…+9 %，PROX +2…+8 %；Pearl 8x RTX 3070：同样算力下 0.660 → 0.677 TH/W。
* 失败后“失败偏移 − 2 步”，绝不从零开始；每一步后保存进度；故障按币种计数，被排除的显卡显示 `OUT 14m` 并会自动恢复。
* Turing（RTX 20xx、GTX 16xx、CMP HX）：结果在挖矿负载上确认 5 分钟；30 分钟内第二次崩溃立即回到稳定偏移；旧的 Turing 配置
  保持 5 分钟一次（`turing-held.json`）。
* “24 小时”保护只计已确认偏移或更低处的失败；一次性参数（`--uv-reset`、`--uv-reset-all`、`--uv-unpause`、`--auto-lock-reset`）
  只在你主动启动时生效一次——不要一直留在飞行表里。

**屏幕：显卡表格**（第 31 节）
* 算力、温度、风扇 %、功耗、功耗上限、每瓦效率、频率 / 锁频、偏移和调校状态、份额、BAD、状态；矿机合计。
* 表头：矿池和矿工名、`net`（区块、任务 id 和存在时间、份额难度和全网难度、ping、过期份额）、`mode`（算法、锁频）、`args`
  （隐藏机密）、最多 5 条最新事件、实时警报。橙色 = 调校在工作，红色 = 需要你处理。
* `--ui table|lines|auto|ascii|unicode`、`--ui-demo`、`--no-color`。日志文件和 HiveOS h-stats 不变。

**HiveOS**
* `--hive-oc-sync`：调好的偏移和锁频以消息发送到 HiveOS 面板；有 API 令牌时（最好放在 `chmod 600` 的文件中：
  `--hive-oc-sync-token-file`）写入该矿机的超频配置，`--split` 的每个组只写自己的显卡（第 32 节）。
* 矿工窗口保持打开：h-run.sh 中循环重启，频繁退出后暂停 10 分钟。
* `--split` 可用于任意币种的飞行表（飞行表的矿池和钱包归入其币种的组）。
* 软件包文件名中只能有一个连字符（`freeforgem-1.8.1_hiveos.tar.gz`）。

**矿池和币种**（第 33 节）
* 被矿池弄坏的会话（矿池降低了份额难度，或回答很晚且过期份额很多）会被替换：`pool: reconnecting`。
* NEB：引擎 nebgpu 0.8.4（Turing 上更快）；没有摘要表的模型不再误报 `NO TABLE`（它需要完整显存频率，`--table-mclock 810` 在此不降低显存）。
* 所有币种都支持 `--api-port`。CMFD 效率以 FW/W 表示。
