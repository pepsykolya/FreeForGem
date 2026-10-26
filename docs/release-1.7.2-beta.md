# FreeForGem 1.7.2-beta — release notes

[English](#english) · [Русский](#русский)

## English

**Fixed in 1.7.2-beta** (from a real incident on an 8x RTX 3070 HiveOS rig, 26.10.2026): undervolt offsets verified at one
core clock lock were applied at another one without a check (Xid 13/43, all GPUs stopped, a HiveOS restart loop of 485 starts);
a GPU that crashed at its own starting offset was never identified and its offset never lowered; one faulting GPU stopped all
GPUs and the miner exited; on HiveOS the offset left by another miner was taken for the owner's. The startup tuning no longer
stops every GPU from mining.

**The known issue of 1.7.1-beta** (offsets verified at another clock lock applied without a check; workaround: re-apply the HiveOS OC
and start once with `--uv-reset`) **is fixed: no workaround is needed any more.**

### Fixes

* **Undervolt profiles per clock lock** (Pearl, NOID, CMFD). An offset holds only at the core clock it was verified at. auto-uv
  keeps one result per GPU, coin / NOID mode and core clock lock: with `--lock-cclock` the lock picks its profile; without it the
  clock under mining load picks it (`1805`, `~1815`, `unlocked`). Switching locks reuses each lock's verified result (no retune);
  a lock without a profile is tuned and the others stay. The start log lists them: `profiles: 1400:+210, 1900:+180; using 1400`.
  `--uv-reset` forgets only the profile of the clocks of this run; `--uv-reset-all` forgets all. A changed starting offset no
  longer forces a new tuning.
* **Results from before 1.7.2** (FreeForGemPear 1.6.x imports, FreeForGem 1.7.0/1.7.1) carry no lock: each becomes the profile of
  the clock it was found at under load and is used only after a check at the clocks of the run (CMFD: the lock in its engine
  identity becomes the key; NOID: only results tuned with `--lock-cclock` are kept).
* **Never an unchecked starting offset over a verified lower one.** "No safe offset above +325 was found; keeping it" kept the
  owner's offset even when the tuning had proven the edge below it (Rig01: +270..+315 verified at 1805 MHz, +325 kept → Xid 13).
  Now the offset verified at these clocks is used; a tuning whose start fails looks for a lower offset that passes.
* **HiveOS owner offset.** The offset put back on exit is the OC profile's *Core Clock* value (`/hive-config/nvidia-oc.conf`), not
  what the card had at start (+325 left by another miner whose exit restore was cut short, instead of +275); such a leftover is
  replaced by the profile value at start. Pearl, NOID, CMFD, and the crash recovery.
* **Which GPU failed.** A CUDA fault on one GPU ends CUDA for the whole process (all workers report "an illegal memory access").
  The miner now names the failing GPU from the driver's Xid events, remembers it in `gpu-faults.json`, restarts itself (Linux /
  HiveOS: in the same process, at most 12 times an hour) and acts on it: a crashed undervolt offset counts as failed (tuned below);
  a crash at the owner's offset 2 starts in a row lowers it (HiveOS profile value, then 60 MHz steps); a GPU with nothing left to
  lower is left out of mining for an hour while the other GPUs mine. A worker that stops alone no longer stops the others.
* **Mining during tuning.** Before, no GPU mined until every GPU was tuned (~25 min on the 8-GPU rig). Now mining starts at once;
  each GPU's checks / tuning run in the background and pause only that GPU (Pearl, NOID, CMFD).
* **Lock changes while mining** (lock raised under the miner, OC apply without `--lock-cclock`): the clock under mining load picks
  the profile again before the undervolt goes back.
* A single `--lock-cclock` value for all GPUs that replaces different per-GPU HiveOS locks is now warned about.

* **The miner fixes a crashing offset itself, without a tuning from scratch.** A crash (Xid) at a reused or kept offset steps
  that GPU back to the verified offset of its lock (or 30 MHz below the crash) after a quick check; a verified offset that fails
  its quick check steps down from it; a tuning runs only when the lock has no verified result. No restart loops: at most 12 own
  restarts an hour; when every GPU is left out the miner waits instead of exiting.
* **Nothing sets an offset after the exit restore** (a check thread still running cannot leave an undervolt behind); the restore
  runs before any log line, so a dead log pipe (`tee` killed by HiveOS `miner stop`) cannot stop it halfway.

* **A failed offset is never applied.** A saved offset that fails its quick check is not kept as the profile's offset: the profile
  holds your offset, paused at these clocks for 24 h; only an offset that passed a check is ever applied without one (found in
  the 1.7.2 soak test: a migrated +270 that failed was applied after the next restart).
* A crash while mining steps the offset back 30 MHz (two default steps), not 15: on the soak rig two RTX 3070 whose +315 had
  passed tuning and checks crashed after hours at 1805 MHz.

### Checks

Release gate `tests/hiveos_scenarios.sh` (one real GPU, fake HiveOS OC profile): lock switching and `--uv-reset` per lock, a
foreign miner's leftover OC, HiveOS OC applies at start and while mining, SIGINT+SIGKILL during the start check and during the
exit restore, an offset above the verified edge, injected GPU faults (also one of two workers), migration of 1.6.x / 1.7.1
files; NOID and CMFD profiles. Unit tests: profile choice, migration, the Rig01 regression, fault records.

## Русский

**Исправлено в 1.7.2-beta** (по реальному инциденту на риге 8x RTX 3070 под HiveOS, 26.10.2026): offset'ы андервольта,
проверенные при одном локе частоты ядра, применялись при другом без проверки (Xid 13/43, остановка всех карт, цикл из 485
перезапусков HiveOS); карта, падавшая на своём стартовом offset, не определялась, и её offset не понижался; сбой одной карты
останавливал все карты и майнер выходил; на HiveOS offset, оставленный другим майнером, принимался за offset владельца.
Стартовый тюнинг больше не останавливает майнинг всех карт.

**Известная проблема 1.7.1-beta** (offset'ы, проверенные при другом локе, применялись без проверки; обход: заново применить разгон
HiveOS и один раз запустить с `--uv-reset`) **исправлена: обход больше не нужен.**

### Исправления

* **Профили андервольта по локу частоты** (Pearl, NOID, CMFD). Offset верен только на той частоте ядра, на которой проверен.
  auto-uv хранит результат для каждой карты, монеты / режима NOID и лока ядра: с `--lock-cclock` профиль выбирает лок; без него —
  частота под нагрузкой майнинга (`1805`, `~1815`, `unlocked`). Переключение локов берёт проверенный результат каждого лока (без
  тюнинга); лок без профиля тюнится, остальные не трогаются. Стартовый лог: `profiles: 1400:+210, 1900:+180; using 1400`.
  `--uv-reset` забывает только профиль частот этого запуска; `--uv-reset-all` — все. Изменившийся стартовый offset больше не
  вызывает повторный тюнинг.
* **Результаты до 1.7.2** (импорт из FreeForGemPear 1.6.x, FreeForGem 1.7.0/1.7.1) без лока: каждый становится профилем частоты,
  на которой найден, и используется только после проверки на частотах запуска (CMFD: лок из идентификатора движка становится
  ключом; NOID: сохраняются только результаты с `--lock-cclock`).
* **Непроверенный стартовый offset больше не оставляется вместо проверенного меньшего.** «No safe offset above +325 was found;
  keeping it» оставлял offset владельца, даже когда тюнинг доказал край ниже него (Rig01: +270..+315 проверены на 1805 МГц,
  оставлен +325 → Xid 13). Теперь используется offset, проверенный на этих частотах; если не проходит сам стартовый offset
  тюнинга, ищется меньший.
* **Offset владельца на HiveOS.** На выходе возвращается *Core Clock* профиля разгона (`/hive-config/nvidia-oc.conf`), а не то, что
  было на карте при старте (+325 — остаток другого майнера, чьё восстановление на выходе прервалось, вместо +275); такой остаток при старте
  заменяется значением профиля. Pearl, NOID, CMFD и восстановление после аварийного выхода.
* **Какая карта упала.** Сбой CUDA на одной карте завершает CUDA всего процесса (все воркеры: «an illegal memory access»). Майнер
  определяет упавшую карту по событиям Xid драйвера, запоминает в `gpu-faults.json`, перезапускает себя (Linux / HiveOS — в том же
  процессе, до 12 раз в час) и действует: упавший андервольт-offset — провальный (тюнинг ниже); падение на offset владельца
  2 старта подряд понижает его (значение профиля HiveOS, затем шаги по 60 МГц); карта, которую некуда понижать, исключается из
  майнинга на час, остальные майнят. Воркер, остановившийся один, больше не останавливает остальные.
* **Майнинг во время тюнинга.** Раньше ни одна карта не майнила, пока не дотюнятся все (~25 мин на риге из 8 карт). Теперь майнинг
  стартует сразу; проверки / тюнинг каждой карты идут в фоне и ставят на паузу только её (Pearl, NOID, CMFD).
* **Смена лока в майнинге** (лок подняли, OC-apply без `--lock-cclock`): профиль заново выбирает частота под нагрузкой, и только
  потом возвращается андервольт.
* Предупреждение, если одно значение `--lock-cclock` на все карты заменяет разные поканальные локи HiveOS.

* **Майнер сам исправляет падающий offset, без тюнинга с нуля.** Падение (Xid) на повторно применённом или оставленном offset
  возвращает карту к проверенному offset её лока (или на 30 МГц ниже падения) после быстрой проверки; проверенный offset, не прошедший
  быструю проверку, понижается по шагу; тюнинг — только если для лока нет проверенного результата. Без циклов перезапуска: не
  больше 12 собственных перезапусков в час; если исключены все карты, майнер ждёт, а не выходит.
* **После восстановления на выходе offset больше никто не ставит** (ещё работающий поток проверки не оставит андервольт);
  восстановление идёт до любой строки лога, поэтому мёртвый канал лога (`tee`, убитый `miner stop` HiveOS) его не прерывает.

* **Не прошедший проверку offset никогда не применяется.** Сохранённый offset, не прошедший быструю проверку, не остаётся offset'ом
  профиля: в профиле ваш offset, пауза на этих частотах 24 ч; без проверки применяется только offset, прошедший проверку (найдено
  при длительном тесте 1.7.2: мигрированный +270 после провала применялся при следующем перезапуске).
* Падение в майнинге понижает offset на 30 МГц (два шага по умолчанию), а не на 15: на тестовом риге две RTX 3070 с +315,
  прошедшим тюнинг и проверки, упали через несколько часов на 1805 МГц.

### Проверки

Release gate `tests/hiveos_scenarios.sh` (одна реальная карта, поддельный профиль разгона HiveOS): смена локов и `--uv-reset` по
локу, остаток разгона чужого майнера, OC-apply HiveOS при старте и в майнинге, SIGINT+SIGKILL во время стартовой проверки и во
время восстановления на выходе, offset выше проверенного края, инъекция сбоя карты (в том числе одного из двух воркеров),
миграция файлов 1.6.x / 1.7.1; профили NOID и CMFD. Юнит-тесты: выбор профиля, миграция, регрессия Rig01, записи сбоев.
