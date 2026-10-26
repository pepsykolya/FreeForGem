# FreeForGem 1.7.1-beta — release notes

[English](#english) · [Русский](#русский)

## English

**Fixed in 1.7.1-beta:** the HiveOS clock lock is put back on exit instead of removed; an OC apply (HiveOS `nvidia-oc` at a
miner start or after an OC change) no longer switches the undervolt off — Pearl, NOID and CMFD put the verified offset back
after a quick check; stopping the miner during a check (HiveOS `miner stop` / `miner restart`) is no longer taken for a failure
(no "died while checking", no 24 h pause of auto-uv). New: `--auto-uv` for Common Foundry (CMFD) on NVIDIA and on AMD Radeon.

### Fixes

* **Clock lock put back on exit** (`--lock-cclock` / `--lock-mclock`). Before, the miner removed the lock on exit and a HiveOS
  rig ran unlocked. Now the lock that was there before comes back: on HiveOS the lock of the OC profile (read from
  `/hive-config/nvidia-oc.conf`), elsewhere the value of `FFG_RESTORE_CCLOCK` / `FFG_RESTORE_MCLOCK` (the FreeForGemPear names
  `FFGP_RESTORE_*` work too; one value or one per GPU, 0 = none); otherwise the clock is reset as before. Also after a crash or
  `kill -9` (the next start). Pearl, NOID and CMFD.
* **OC apply while mining.** HiveOS `nvidia-oc` writes the *Core Clock* value of the profile at every miner start and OC change,
  **0 when the field is empty** — before, this switched the undervolt off until the next restart. Now an offset read back as 0,
  as the HiveOS profile value or as your own starting offset counts as an OC apply (at most 4 times an hour; any other value is
  your own choice and the miner stands down on that GPU):
  * **Pearl:** the verified offset is set again after a quick check (mining on that GPU pauses for ~30 s; a failure steps back).
  * **NOID:** the offset is read back every 30 s; a tuned NOID offset comes back after a 30 s stress check with mining on that GPU
    paused; the base 0 (without `--auto-uv`) or the `--uv-offset` value is set again at once — a Pearl offset put back by the OC
    would make NOID compute silently wrong results.
  * **CMFD:** the engine of that GPU stops, the saved CMFD offset gets the quick check on the model + the official test vector,
    and the engine restarts at it.
  * A check during which an OC apply reset the offset is repeated at the right offset (Pearl, NOID, CMFD).
* **A stop during a check is not a failure.** HiveOS `miner stop` / `miner restart` sends Ctrl+C and then closes the screen
  session (a second signal). Before, the offset under check stayed as a "crash point" (the next start: "died while checking …,
  not tuning") or the start quick check was counted as failed (auto-uv paused for 24 h). Fixed for Pearl, NOID (the check
  process killed by the same Ctrl+C is no verdict) and CMFD.
* **Low-RAM rigs (Pearl):** a re-check while mining takes its RAM first and only then pauses its GPU, so a GPU waiting for RAM
  keeps mining.

### New

* **`--auto-uv` for CMFD on NVIDIA:** a CMFD offset of its own per card, every step checked bit-for-bit on the real model by the
  engine's worker + the official Common Foundry test vector; saved in `uv.json` (section `cmfd`). CMP 50HX (225 W, 1410 MHz):
  13.10 FW/s at 216 W → 13.21 at 196 W (+11 % FW/kWh). MANUAL.md, section 13.
* **`--auto-uv` for CMFD on AMD Radeon (Windows):** amduv with a separate CMFD point, profiles `UV_GOAL` = `eff` / `bal` /
  `max`; RX 6800 XT stock 10.0 FW/s at 280 W → `bal` 9.1 FW/s at 193 W (+32 % FW/W). MANUAL.md, section 14.
* MANUAL: CMP cards (CMP 50HX: not recommended for Pearl, fine for CMFD; no CMP unlocking).

### Checked (RTX 3070, Shahtyor GPU1)

* Clock lock: Ctrl-C / SIGTERM / `kill -9` + next start → the HiveOS profile lock 1200/810 is back; `FFG_RESTORE_*` and
  `FFGP_RESTORE_*` → 1250/810, 1350/810; without either → reset (as before). The same with `--algo noid`.
* Pearl: OC apply mid-run (0, profile value +30) → +150 back after a quick check, a hand value +45 → stands down; OC apply
  during the start check → check repeated; HiveOS-like stop during the start check → no marker, no pause. Pool 3 min:
  **66.6 TH/s @ 103 W, 9/9 accepted**.
* NOID: self-test (eff + speed) passed; OC apply mid-run (0, +30) → +195 back after the stress check, +45 by hand → stands down;
  without `--auto-uv` the owner's +175 put back by an "OC" → 0 again; OC apply during the start check → repeated; stop during
  the check → no marker, no pause. Pool 3 min (InnovLab, eff @1400 MHz, `--auto-uv` +195): **34.8 MH/s @ 72 W (0.482 MH/W),
  37 accepted / 0 rejected**.
* CMFD (Aria, 135 W, saved +195): stop during the check → no marker, no worker left; 3 min **14.2–14.6 FW/s @ 135 W**;
  OC apply mid-run → engine stopped, quick check +195 ok (14.65 FW/s), engine restarted at +195; +45 by hand → stands down.
  (No CMFD shares in these short runs: the Aria share rate at 14 FW/s is low.)
* Windows build: Pearl self-test passed (RTX 4070 Laptop).

The Pearl libraries are byte-identical to 1.6.2/1.7.0-beta, the NOID libraries to 1.7.0-beta.

### Files

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.1beta.tar.gz` — HiveOS custom miner (Installation URL) | `41b0607f83915f29316f735f31c5625841328476af453b49758d759060093341` |
| `freeforgem-1.7.1-beta-linux-x86_64.tar.gz` — Linux x86_64 (glibc 2.31+) | `a5acde427075f4b1347b350a5a4b1e1b4156df3e4e5c67e452fbcc68ac9cb880` |
| `freeforgem-1.7.1-beta-windows-x64.zip` — Windows 10/11 x64 | `98acad2ce6b21eaf9a0f0031a618d0e7000f61d3e0cd00dffc260bc9a3fbe541` |

HiveOS Installation URL: `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.1-beta/freeforgem-1.7.1beta.tar.gz`
(the same flight sheet as 1.7.0-beta, only the URL changes).

---

## Русский

**Исправлено в 1.7.1-beta:** лок частот HiveOS при выходе возвращается, а не снимается; применение разгона (HiveOS `nvidia-oc`
при старте майнера или смене OC) больше не выключает андервольт — Pearl, NOID и CMFD ставят проверенный offset обратно после
быстрой проверки; остановка майнера во время проверки (`miner stop` / `miner restart` в HiveOS) больше не считается провалом
(нет «died while checking», нет паузы auto-uv на 24 ч). Новое: `--auto-uv` для Common Foundry (CMFD) на NVIDIA и AMD Radeon.

### Исправления

* **Лок частот возвращается при выходе** (`--lock-cclock` / `--lock-mclock`). Раньше майнер при выходе снимал лок, и риг HiveOS
  оставался без него. Теперь возвращается прежний лок: в HiveOS — из профиля разгона (`/hive-config/nvidia-oc.conf`), в других
  системах — из `FFG_RESTORE_CCLOCK` / `FFG_RESTORE_MCLOCK` (имена FreeForGemPear `FFGP_RESTORE_*` тоже работают; одно значение
  или по карте, 0 = лока не было); иначе сброс, как раньше. И после сбоя / `kill -9` (следующим запуском). Pearl, NOID и CMFD.
* **Применение разгона во время майнинга.** `nvidia-oc` в HiveOS при каждом старте майнера и смене OC пишет *Core Clock* из
  профиля, **0 при пустом поле** — раньше это выключало андервольт до перезапуска. Теперь offset, прочитанный как 0, как
  значение профиля HiveOS или как ваш исходный offset, считается применением разгона (не чаще 4 раз в час; любое другое
  значение — ваш выбор, майнер отступает на этой карте):
  * **Pearl:** проверенный offset ставится снова после быстрой проверки (майнинг на карте встаёт на ~30 с; ошибка — шаг назад).
  * **NOID:** offset перечитывается каждые 30 с; подобранный NOID-offset возвращается после 30-секундной stress-проверки с паузой
    майнинга на этой карте; база 0 (без `--auto-uv`) или значение `--uv-offset` ставится сразу — offset от Pearl, возвращённый
    разгоном, дал бы тихо неверные результаты NOID.
  * **CMFD:** движок этой карты останавливается, сохранённый CMFD-offset проходит быструю проверку на модели + официальный
    тест-вектор, движок перезапускается на нём.
  * Проверка, во время которой разгон сбросил offset, повторяется на нужном offset (Pearl, NOID, CMFD).
* **Остановка во время проверки — не провал.** `miner stop` / `miner restart` в HiveOS шлёт Ctrl+C, а потом закрывает
  screen-сессию (второй сигнал). Раньше проверяемый offset оставался «точкой сбоя» (следующий старт: «died while checking …,
  not tuning») или стартовая быстрая проверка считалась проваленной (auto-uv на паузе 24 ч). Исправлено для Pearl, NOID
  (процесс проверки, убитый тем же Ctrl+C, — не вердикт) и CMFD.
* **Риги с малым ОЗУ (Pearl):** перепроверка в майнинге сначала получает память и только потом ставит карту на паузу — карта,
  ждущая ОЗУ, продолжает майнить.

### Новое

* **`--auto-uv` для CMFD на NVIDIA:** свой CMFD-offset на каждую карту, каждый шаг проверяется побитно на настоящей модели воркером
  движка + официальный тест-вектор Common Foundry; хранится в `uv.json` (раздел `cmfd`). CMP 50HX (225 Вт, 1410 МГц): 13,10 FW/s
  при 216 Вт → 13,21 при 196 Вт (+11 % FW/kWh). MANUAL.md, раздел 13.
* **`--auto-uv` для CMFD на AMD Radeon (Windows):** amduv с отдельной точкой для CMFD, профили `UV_GOAL` = `eff` / `bal` / `max`;
  RX 6800 XT сток 10,0 FW/s при 280 Вт → `bal` 9,1 FW/s при 193 Вт (+32 % FW/Вт). MANUAL.md, раздел 14.
* MANUAL: карты CMP (CMP 50HX для Pearl не рекомендуется, для CMFD подходит; разблокировки CMP нет).

### Проверено (RTX 3070, Шахтёр GPU1)

* Лок частот: Ctrl-C / SIGTERM / `kill -9` + следующий старт → лок профиля HiveOS 1200/810 на месте; `FFG_RESTORE_*` и
  `FFGP_RESTORE_*` → 1250/810, 1350/810; без них → сброс (как раньше). То же с `--algo noid`.
* Pearl: разгон посреди майнинга (0, значение профиля +30) → +150 возвращён после быстрой проверки, +45 вручную → отступил;
  разгон во время стартовой проверки → проверка повторена; остановка как в HiveOS во время проверки → маркера нет, паузы нет.
  Пул 3 мин: **66,6 TH/s @ 103 Вт, 9/9 принято**.
* NOID: self-test (eff + speed) пройден; разгон посреди майнинга (0, +30) → +195 возвращён после stress-проверки, +45 вручную →
  отступил; без `--auto-uv` возвращённые «разгоном» +175 владельца → снова 0; разгон во время стартовой проверки → повторена;
  остановка во время проверки → маркера нет, паузы нет. Пул 3 мин (InnovLab, eff @1400 МГц, `--auto-uv` +195): **34,8 MH/s
  @ 72 Вт (0,482 MH/Вт), 37 принято / 0 отклонено**.
* CMFD (Aria, 135 Вт, сохранённый +195): остановка во время проверки → маркера нет, воркеров не осталось; 3 мин **14,2–14,6 FW/s
  @ 135 Вт**; разгон посреди майнинга → движок остановлен, быстрая проверка +195 ok (14,65 FW/s), движок перезапущен на +195;
  +45 вручную → отступил. (Шар CMFD в этих коротких прогонах нет: на Aria при 14 FW/s шары редкие.)
* Windows-сборка: Pearl self-test пройден (RTX 4070 Laptop).

Библиотеки Pearl побайтно те же, что в 1.6.2/1.7.0-beta, библиотеки NOID — те же, что в 1.7.0-beta.

### Файлы

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.7.1beta.tar.gz` — кастомный майнер HiveOS (Installation URL) | `41b0607f83915f29316f735f31c5625841328476af453b49758d759060093341` |
| `freeforgem-1.7.1-beta-linux-x86_64.tar.gz` — Linux x86_64 (glibc 2.31+) | `a5acde427075f4b1347b350a5a4b1e1b4156df3e4e5c67e452fbcc68ac9cb880` |
| `freeforgem-1.7.1-beta-windows-x64.zip` — Windows 10/11 x64 | `98acad2ce6b21eaf9a0f0031a618d0e7000f61d3e0cd00dffc260bc9a3fbe541` |

Ссылка для HiveOS: `https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.1-beta/freeforgem-1.7.1beta.tar.gz`
(полётный лист тот же, что для 1.7.0-beta, меняется только ссылка).
