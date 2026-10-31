# FreeForGem 1.7.5 — release notes

[English](#english) · [Русский](#русский)

## English

Files: `freeforgem-1.7.5.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.5/freeforgem-1.7.5.tar.gz`),
`freeforgem-1.7.5-linux-x86_64.tar.gz`, `freeforgem-1.7.5-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.5.tar.gz` | `d2f3eabd2f0e807fd22ff98f444ffb08e3df119b07b8bac60189182a2df76185` |
| `freeforgem-1.7.5-linux-x86_64.tar.gz` | `56be38d7b455ee238582fd1de1437424aee6ab8b4c38e93b3188e0a21f9a49fb` |
| `freeforgem-1.7.5-windows-x64.zip` | `eaafdd551548851eed29252f53e99aefa41803fd0362409909224f8da91464d1` |

**New: idle mode for Windows** (Pearl) — mine only while nobody uses the PC. Set `IDLE=1` in the start file (`--idle`).
"In use" is keyboard / mouse / gamepad input, a fullscreen game or video, other programs at work (more than 25 % of the CPU,
another program on the GPU, any video decoding: a film, a stream, a call) or a program from `ALWAYS_PAUSE`; other miners
(xmrig, SRBMiner, lolMiner, T-Rex, Rigel ...) do not count. A locked screen
counts as idle. While the PC is in use the miner stops within a second (`ACTIVE=pause`, default: the GPU gets its own clocks
and offset back) or keeps a light load you choose (`ACTIVE=light:30` = 30 % of the time, on the verified mining clocks).
Mining comes back after `IDLE_MINUTES` (default 5) without use. The dev fee counts the mining time only. MANUAL.md, section 18.

**New: new-version notice.** About a minute after the start and every ~12 h the miner asks GitHub for the latest release; a
newer one is shown in the log and in HiveOS next to the version and TH/W (`1.7.5 0.652 TH/W, new 1.7.6`). Nothing is
downloaded or installed; `--no-update-check` turns it off, `--check-update` checks once.

**Fixes**
* auto-uv: a tuned offset of this miner left on the card (a run cut by a power failure, an older miner) is no longer taken for
  the owner's offset; the offset recorded before the tuning goes back. On one laptop the leftover came back after every reboot.
* auto-uv: the check queue counts a running check only with the RAM it has not taken yet, and the log says
  `waits its turn for a check (mining meanwhile)` instead of `waits for RAM` (the GPU mines while it waits).

Tests on four Windows PCs (RTX 3080 Ti, RTX 3070, RTX 3070 Laptop, RTX 4070 Laptop; locked clocks + auto-uv): start while
in use, 5 pause / resume cycles, real mouse input, Ctrl+C while paused and while mining, a crash in either state and a
restart, start.ps1 with IDLE/ACTIVE, light load — every time the card went back to its own offset and clocks while paused
(e.g. 3080 Ti: +495 / 1560 MHz / 200 W -> +0 / 210 MHz / 15 W) and to the tuned ones after. 8x RTX 3070 (HiveOS): unchanged
mining, saved profiles applied.

**Tip — NOCK wallet:** if the Chrome Web Store says the Iris wallet extension "is not available" (region), install the same
official build from GitHub: `github.com/nockbox/iris/releases` → `iris-wallet.zip` → unpack → `chrome://extensions` →
Developer mode → Load unpacked.

## Русский

Файлы: `freeforgem-1.7.5.tar.gz` (ссылка для установки HiveOS
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.5/freeforgem-1.7.5.tar.gz`),
`freeforgem-1.7.5-linux-x86_64.tar.gz`, `freeforgem-1.7.5-windows-x64.zip`. Сверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.7.5.tar.gz` | `d2f3eabd2f0e807fd22ff98f444ffb08e3df119b07b8bac60189182a2df76185` |
| `freeforgem-1.7.5-linux-x86_64.tar.gz` | `56be38d7b455ee238582fd1de1437424aee6ab8b4c38e93b3188e0a21f9a49fb` |
| `freeforgem-1.7.5-windows-x64.zip` | `eaafdd551548851eed29252f53e99aefa41803fd0362409909224f8da91464d1` |

**Новое: режим простоя для Windows** (Pearl) — майнить, только пока компьютером никто не пользуется. `IDLE=1` в стартовом файле
(`--idle`). «Занят» — ввод с клавиатуры, мыши, геймпада, полноэкранная игра или видео, работа других программ (больше 25 %
процессора, другая программа на видеокарте, любое декодирование видео: фильм, стрим, звонок) или программа из `ALWAYS_PAUSE`; другие
майнеры (xmrig, SRBMiner, lolMiner, T-Rex, Rigel ...) не считаются.
Заблокированный экран — простой. Пока компьютер занят, майнер останавливается меньше чем за секунду (`ACTIVE=pause`, по умолчанию:
видеокарте возвращаются её собственные частоты и offset) или держит выбранную вами лёгкую нагрузку (`ACTIVE=light:30` — 30 %
времени, на проверенных майнинговых частотах). Майнинг возвращается через `IDLE_MINUTES` (по умолчанию 5) без использования.
Комиссия считается только по времени майнинга. MANUAL.md, раздел 18.

**Новое: уведомление о новой версии.** Через минуту после старта и раз в ~12 ч майнер спрашивает у GitHub последний релиз;
новая версия видна в логе и в HiveOS рядом с версией и TH/W (`1.7.5 0.652 TH/W, new 1.7.6`). Ничего не скачивается и не
устанавливается; `--no-update-check` выключает, `--check-update` проверяет один раз.

**Исправления**
* auto-uv: оставленный на карте подобранный offset самого майнера (запуск, прерванный отключением света, старый майнер) больше
  не принимается за ваш; возвращается offset, записанный до подбора. На одном ноутбуке этот остаток возвращался после каждой
  перезагрузки.
* auto-uv: очередь проверок учитывает идущую проверку только той памятью, которую она ещё не заняла, а в логе вместо
  `waits for RAM` — `waits its turn for a check (mining meanwhile)` (карта в очереди майнит).

Тесты на четырёх Windows-ПК (RTX 3080 Ti, RTX 3070, RTX 3070 Laptop, RTX 4070 Laptop; лок частот + auto-uv): старт при занятом
ПК, 5 циклов пауза/работа, настоящая мышь, Ctrl+C в паузе и в майнинге, падение в обоих состояниях и перезапуск, start.ps1 с
IDLE/ACTIVE, лёгкая нагрузка — в паузе карта каждый раз получала свои offset и частоты (например 3080 Ti: +495 / 1560 МГц /
200 Вт → +0 / 210 МГц / 15 Вт), после — подобранные. 8×RTX 3070 (HiveOS): майнинг без изменений, профили применяются.

**Совет — кошелёк NOCK:** если Chrome Web Store пишет, что расширение Iris «недоступно» (регион), установите ту же официальную
сборку с GitHub: `github.com/nockbox/iris/releases` → `iris-wallet.zip` → распаковать → `chrome://extensions` → Режим
разработчика → Загрузить распакованное.
