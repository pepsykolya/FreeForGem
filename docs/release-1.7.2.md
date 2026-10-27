# FreeForGem 1.7.2 — release notes

[English](#english) · [Русский](#русский)

## English

Files: `freeforgem-1.7.2.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2/freeforgem-1.7.2.tar.gz`),
`freeforgem-1.7.2-linux-x86_64.tar.gz`, `freeforgem-1.7.2-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.2.tar.gz` | `0295d630b91a871bd40b95bc8a92e16afa66ca01f1cc30abfa9983e0546a483c` |
| `freeforgem-1.7.2-linux-x86_64.tar.gz` | `6b00ab31140b8f1190b8e4701e65e24997a69c8f7a5d37a12a4caff6ecd81bdb` |
| `freeforgem-1.7.2-windows-x64.zip` | `b6ec88b86bdea4cd51c6026294afa963bca91486063cc7117b1e1c5dc471377d` |
The beta history is in [release-1.7.2-beta.md](release-1.7.2-beta.md).

**Fixed in 1.7.2** (from a real incident on an 8x RTX 3070 HiveOS rig, 26.10.2026): undervolt offsets verified at one core clock
lock were applied at another one without a check (Xid 13/43, all GPUs stopped, a HiveOS restart loop of 485 starts); a GPU that
crashed at its own starting offset was never identified and its offset never lowered; one faulting GPU stopped all GPUs and the
miner exited; on HiveOS the offset left by another miner was taken for the owner's. The startup tuning no longer stops every GPU
from mining.

**The known issue of 1.7.1** (offsets verified at another clock lock applied without a check; workaround: re-apply the HiveOS OC
and start once with `--uv-reset`) **is fixed: no workaround is needed any more.**

### Undervolt profiles per clock lock

* **One profile per lock** (Pearl, NOID, CMFD). An offset holds only at the core clock it was verified at. auto-uv keeps one result
  per GPU, coin / NOID mode and core clock lock: with `--lock-cclock` the lock picks its profile; without it the clock under mining
  load picks it (`1805`, `~1815`, `unlocked`). Switching locks reuses each lock's verified result (no retune); a lock without a
  profile is tuned once and the others stay. The start log lists them: `profiles: 1400:+210, 1900:+180; using 1400`.
* `--uv-reset` forgets only the profile of the current lock (and coin / mode); `--uv-reset-all` forgets all. A self-restart after a
  crash (and on Windows a restart by start.ps1) drops both options, so a flight sheet that still has them does not wipe the
  profiles at every restart. A changed starting offset no longer forces a new tuning.
* **Results from before 1.7.2** (FreeForGemPear 1.6.x imports, FreeForGem 1.7.0/1.7.1) carry no lock: each becomes the profile of
  the clock it was found at under load and is used only after a check at the clocks of the run (CMFD: the lock in its engine
  identity becomes the key; NOID: only results tuned with `--lock-cclock` are kept).
* **Lock changes while mining** (lock raised under the miner, OC apply without `--lock-cclock`): the clock under mining load picks
  the profile again before the undervolt goes back. A single `--lock-cclock` value for all GPUs that replaces different per-GPU
  HiveOS locks is warned about.

### Offsets that are never applied unchecked

* **Never an unchecked starting offset over a verified lower one.** "No safe offset above +325 was found; keeping it" kept the
  owner's offset even when the tuning had proven the edge below it (+270..+315 verified at 1805 MHz, +325 kept → Xid 13). Now the
  offset verified at these clocks is used; a tuning whose start fails looks for a lower offset that passes (30 MHz steps).
* **A failed offset is never applied.** A saved offset that fails its quick check is not kept as the profile's offset: the profile
  holds your offset, paused at these clocks for 24 h; only an offset that passed a check is ever applied without one.
* **HiveOS owner offset.** The offset put back on exit is the OC profile's *Core Clock* value (`/hive-config/nvidia-oc.conf`), not
  what the card had at start (+325 left by another miner whose exit restore was cut short, instead of +275); such a leftover is
  replaced by the profile value at start. Pearl, NOID, CMFD, and the crash recovery.
* **Nothing sets an offset after the exit restore** (a check thread still running cannot leave an undervolt behind); the restore
  runs before any log line, so a dead log pipe (`tee` killed by HiveOS `miner stop`) cannot stop it halfway.

### Crashes while mining

* **Which GPU failed.** A CUDA fault on one GPU ends CUDA for the whole process (all workers report "an illegal memory access").
  The miner now names the failing GPU from the driver's Xid events, remembers it in `gpu-faults.json` and restarts itself (Linux /
  HiveOS: in the same process, HiveOS does not count it, at most 12 times an hour; Windows: start.ps1 restarts it). A worker that
  stops alone no longer stops the others.
* **The miner fixes a crashing offset itself, without a tuning from scratch.** A crash (Xid) at a reused or kept offset steps that
  GPU back to the verified offset of its lock (or 30 MHz below the crash) after a quick check; a verified offset that fails its
  quick check steps down from it; a tuning runs only when the lock has no verified result. A crash at the owner's offset 2 starts
  in a row lowers it (HiveOS profile value, then 60 MHz steps); a GPU with nothing left to lower is left out of mining for an hour
  while the other GPUs mine; when every GPU is left out the miner waits instead of exiting.

### Tuning

* **Mining during tuning.** Before, no GPU mined until every GPU was tuned (~25 min on the 8-GPU rig). Now mining starts at once;
  each GPU's checks / tuning run in the background and pause only that GPU (Pearl, NOID, CMFD). 8x RTX 3070: about 11 min.
* **Faster tuning on RTX 3070 / 3060 Ti**: the search starts near the edge by the core lock — Pearl 210 + 0.15 x (1805 - lock),
  +150..+300 (+210 at 1805, +270 at 1410); NOID eff 105 + 0.25 x (lock - 1400), +90..+210 (+105 at 1400, +150 at 1560); NOID
  speed 240 - 0.12 x (lock - 1500), +195..+270 (+240 at 1500, +210 at 1750); CMFD +150; RTX 3080/3090 +180 (NOID: at 1560). A
  start that fails steps down 30 MHz; NOID (both modes) climbs in 30 MHz steps and checks the 15 MHz step below the first
  failure; the climb stops at the voltage floor. `--uv-min` overrides the start.
* **Progress and colours**: per-GPU state in the stats line, a summary about every minute, a RESULT line per GPU; coloured log
  (`--no-color` / `NO_COLOR=1` turn it off); h-stats.sh strips the colour codes.
* **NOID**: a result less efficient than the base keeps the base ("no gain at this clock"); MH/W is measured only under the GPU load.

### HiveOS and NOID pool

* **HiveOS reboot at a high load average** (NOID tuning, 4C/8T Xeon, 8 GPUs): the NOID GPU threads no longer busy-wait for the
  GPU, the CPU reference of the checks runs once per machine at a time on CPU threads - 1 at nice 10 and is cached; the tuning
  summary shows the load average (Linux) with a WARNING above 2 x the CPU threads.
* **NOID stale shares**: a share of a job the pool has voided (pause / clean job) is dropped before it is sent; `clean: false`
  keeps the older job valid; shares go out within 20 ms. At the start the innovlab vardiff target is easy and the replies lag:
  at most 12 unanswered shares per connection, the best go first, easier surplus ones are skipped, a block share is always
  sent. Stats line: `rej N (stale N, skipped N)`. Test against a mock pool that reproduces the innovlab start (easy target,
  slow replies, a new job every 15 s; 1 GPU, 5 min): beta5 2918 sent, 148 accepted, 1847 stale, pool lag up to 93 s; 1.7.2
  2009 sent, 1767 accepted, 229 stale, 628 skipped, lag at most 1.8 s. innovlab acknowledges `mining.suggest_difficulty` but
  keeps its start target, so none is requested.
* **HiveOS dashboard**: the rig efficiency (TH/W for Pearl, MH/W for NOID) is shown next to the miner version.
* **NOID recommendation on RTX 30** (8x RTX 3070 rig, `--auto-uv`, MANUAL.md section 15): **eff `--lock-cclock 1300`** for the
  best MH/W (257.0 MH/s, 523 W, 0.491 MH/W on 1.7.2), **speed `--lock-cclock 1500`** for more hashrate (336.0 MH/s, 710 W,
  0.473 MH/W: +31 % MH/s for +36 % power).

### Checks

Release gate `tests/hiveos_scenarios.sh` (one real GPU, fake HiveOS OC profile): lock switching and `--uv-reset` per lock, a
foreign miner's leftover OC, HiveOS OC applies at start and while mining, SIGINT+SIGKILL during the start check and during the
exit restore, an offset above the verified edge, injected GPU faults (also one of two workers), migration of 1.6.x / 1.7.1
files; NOID and CMFD profiles. Unit tests: profile choice, migration, the 8x RTX 3070 regression, fault records, one-shot
options dropped on a restart.

## Русский

Файлы: `freeforgem-1.7.2.tar.gz` (Installation URL для HiveOS
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.2/freeforgem-1.7.2.tar.gz`),
`freeforgem-1.7.2-linux-x86_64.tar.gz`, `freeforgem-1.7.2-windows-x64.zip`. Сверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.7.2.tar.gz` | `0295d630b91a871bd40b95bc8a92e16afa66ca01f1cc30abfa9983e0546a483c` |
| `freeforgem-1.7.2-linux-x86_64.tar.gz` | `6b00ab31140b8f1190b8e4701e65e24997a69c8f7a5d37a12a4caff6ecd81bdb` |
| `freeforgem-1.7.2-windows-x64.zip` | `b6ec88b86bdea4cd51c6026294afa963bca91486063cc7117b1e1c5dc471377d` |
История бет — в [release-1.7.2-beta.md](release-1.7.2-beta.md).

**Исправлено в 1.7.2** (по реальному инциденту на риге 8x RTX 3070 под HiveOS, 26.10.2026): offset'ы андервольта, проверенные при
одном локе частоты ядра, применялись при другом без проверки (Xid 13/43, остановка всех карт, цикл из 485 перезапусков HiveOS);
карта, падавшая на своём стартовом offset, не определялась, и её offset не понижался; сбой одной карты останавливал все карты и
майнер выходил; на HiveOS offset, оставленный другим майнером, принимался за offset владельца. Стартовый тюнинг больше не
останавливает майнинг всех карт.

**Известная проблема 1.7.1** (offset'ы, проверенные при другом локе, применялись без проверки; обход: заново применить разгон
HiveOS и один раз запустить с `--uv-reset`) **исправлена: обход больше не нужен.**

### Профили андервольта по локу частоты

* **Профиль на каждый лок** (Pearl, NOID, CMFD). Offset верен только на той частоте ядра, на которой проверен. auto-uv хранит
  результат для каждой карты, монеты / режима NOID и лока ядра: с `--lock-cclock` профиль выбирает лок; без него — частота под
  нагрузкой майнинга (`1805`, `~1815`, `unlocked`). Переключение локов берёт проверенный результат каждого лока (без тюнинга); лок
  без профиля тюнится один раз, остальные не трогаются. Стартовый лог: `profiles: 1400:+210, 1900:+180; using 1400`.
* `--uv-reset` забывает только профиль текущего лока (и монеты / режима); `--uv-reset-all` — все. Перезапуск майнера после падения
  (и в Windows перезапуск через start.ps1) убирает обе опции, поэтому полётный лист, где они остались, не стирает профили при
  каждом перезапуске. Изменившийся стартовый offset больше не вызывает повторный тюнинг.
* **Результаты до 1.7.2** (импорт из FreeForGemPear 1.6.x, FreeForGem 1.7.0/1.7.1) без лока: каждый становится профилем частоты,
  на которой найден, и используется только после проверки на частотах запуска (CMFD: лок из идентификатора движка становится
  ключом; NOID: сохраняются только результаты с `--lock-cclock`).
* **Смена лока в майнинге** (лок подняли, OC-apply без `--lock-cclock`): профиль заново выбирает частота под нагрузкой, и только
  потом возвращается андервольт. Предупреждение, если одно значение `--lock-cclock` на все карты заменяет разные поканальные
  локи HiveOS.

### Offset'ы, которые не применяются без проверки

* **Непроверенный стартовый offset больше не оставляется вместо проверенного меньшего.** «No safe offset above +325 was found;
  keeping it» оставлял offset владельца, даже когда тюнинг доказал край ниже него (+270..+315 проверены на 1805 МГц, оставлен
  +325 → Xid 13). Теперь используется offset, проверенный на этих частотах; если не проходит сам стартовый offset тюнинга,
  ищется меньший (шагами 30 МГц).
* **Не прошедший проверку offset никогда не применяется.** Сохранённый offset, не прошедший быструю проверку, не остаётся
  offset'ом профиля: в профиле ваш offset, пауза на этих частотах 24 ч; без проверки применяется только offset, прошедший проверку.
* **Offset владельца на HiveOS.** На выходе возвращается *Core Clock* профиля разгона (`/hive-config/nvidia-oc.conf`), а не то, что
  было на карте при старте (+325 — остаток другого майнера, чьё восстановление на выходе прервалось, вместо +275); такой остаток
  при старте заменяется значением профиля. Pearl, NOID, CMFD и восстановление после аварийного выхода.
* **После восстановления на выходе offset больше никто не ставит** (ещё работающий поток проверки не оставит андервольт);
  восстановление идёт до любой строки лога, поэтому мёртвый канал лога (`tee`, убитый `miner stop` HiveOS) его не прерывает.

### Падения в майнинге

* **Какая карта упала.** Сбой CUDA на одной карте завершает CUDA всего процесса (все воркеры: «an illegal memory access»). Майнер
  определяет упавшую карту по событиям Xid драйвера, запоминает в `gpu-faults.json` и перезапускает себя (Linux / HiveOS — в том же
  процессе, HiveOS его не считает, не больше 12 раз в час; Windows — перезапускает start.ps1). Воркер, остановившийся один, больше
  не останавливает остальные.
* **Майнер сам исправляет падающий offset, без тюнинга с нуля.** Падение (Xid) на повторно применённом или оставленном offset
  возвращает карту к проверенному offset её лока (или на 30 МГц ниже падения) после быстрой проверки; проверенный offset, не
  прошедший быструю проверку, понижается по шагу; тюнинг — только если для лока нет проверенного результата. Падение на offset
  владельца 2 старта подряд понижает его (значение профиля HiveOS, затем шаги по 60 МГц); карта, которую некуда понижать,
  исключается из майнинга на час, остальные майнят; если исключены все карты, майнер ждёт, а не выходит.

### Тюнинг

* **Майнинг во время тюнинга.** Раньше ни одна карта не майнила, пока не дотюнятся все (~25 мин на риге из 8 карт). Теперь майнинг
  стартует сразу; проверки / тюнинг каждой карты идут в фоне и ставят на паузу только её (Pearl, NOID, CMFD). 8x RTX 3070: около 11 мин.
* **Быстрее тюнинг на RTX 3070 / 3060 Ti**: поиск начинается рядом с краем по локу ядра — Pearl 210 + 0,15 x (1805 - лок),
  +150..+300 (+210 при 1805, +270 при 1410); NOID eff 105 + 0,25 x (лок - 1400), +90..+210 (+105 при 1400, +150 при 1560); NOID
  speed 240 - 0,12 x (лок - 1500), +195..+270 (+240 при 1500, +210 при 1750); CMFD +150; RTX 3080/3090 +180 (NOID: при 1560).
  Старт, не прошедший проверку, понижается на 30 МГц; NOID (оба режима) поднимается шагами по 30 МГц и проверяет шаг 15 МГц под
  первым провалом; подъём останавливается на пороге напряжения. `--uv-min` задаёт старт.
* **Прогресс и цвета**: состояние карты в строке статистики, сводка раз в минуту, строка RESULT по каждой карте; цветной лог
  (`--no-color` / `NO_COLOR=1` выключают); h-stats.sh убирает коды цвета.
* **NOID**: результат менее эффективный, чем база, не применяется ("no gain at this clock"); MH/W меряется только под нагрузкой GPU.

### HiveOS и пул NOID

* **Перезагрузка HiveOS при высоком load average** (тюнинг NOID, Xeon 4C/8T, 8 карт): потоки GPU в NOID больше не ждут GPU
  активно, CPU-сверка проверок идёт одна на машину за раз на (потоки CPU - 1) с nice 10 и кэшируется; сводка тюнинга показывает
  load average (Linux) с WARNING выше 2 x числа потоков CPU.
* **Устаревшие шары NOID**: шара задания, которое пул отменил (pause / clean job), отбрасывается до отправки; `clean: false`
  оставляет старое задание действующим; шары уходят за 20 мс. На старте цель vardiff innovlab лёгкая, и ответы отстают: не
  больше 12 шар без ответа на соединение, лучшие уходят первыми, лишние более лёгкие пропускаются, шара блока отправляется
  всегда. Строка статистики: `rej N (stale N, skipped N)`. Тест на мок-пуле, повторяющем старт innovlab (лёгкая цель,
  медленные ответы, новое задание каждые 15 с; 1 карта, 5 мин): beta5 — отправлено 2918, принято 148, stale 1847, отставание
  пула до 93 с; 1.7.2 — отправлено 2009, принято 1767, stale 229, skipped 628, отставание не больше 1,8 с. innovlab принимает
  `mining.suggest_difficulty`, но стартовую цель не меняет, поэтому она не запрашивается.
* **Панель HiveOS**: эффективность рига (TH/Вт для Pearl, MH/Вт для NOID) рядом с версией майнера.
* **Рекомендация NOID на RTX 30** (риг 8x RTX 3070, `--auto-uv`, MANUAL.md раздел 15): **eff `--lock-cclock 1300`** — лучшие
  MH/Вт (257,0 MH/s, 523 Вт, 0,491 MH/Вт на 1.7.2), **speed `--lock-cclock 1500`** — больше хешрейта (336,0 MH/s, 710 Вт,
  0,473 MH/Вт: +31 % MH/s за +36 % мощности).

### Проверки

Release gate `tests/hiveos_scenarios.sh` (одна реальная карта, поддельный профиль разгона HiveOS): смена локов и `--uv-reset` по
локу, остаток разгона чужого майнера, OC-apply HiveOS при старте и в майнинге, SIGINT+SIGKILL во время стартовой проверки и во
время восстановления на выходе, offset выше проверенного края, инъекция сбоя карты (в том числе одного из двух воркеров),
миграция файлов 1.6.x / 1.7.1; профили NOID и CMFD. Юнит-тесты: выбор профиля, миграция, регрессия 8x RTX 3070, записи сбоев,
снятие одноразовых опций при перезапуске.
