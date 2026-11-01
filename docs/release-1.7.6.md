# FreeForGem 1.7.6 — release notes

[English](#english) · [Русский](#русский)

## English

Files: `freeforgem-1.7.6.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/freeforgem-1.7.6.tar.gz`),
`freeforgem-1.7.6-linux-x86_64.tar.gz`, `freeforgem-1.7.6-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

Video tutorial: [English](https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/FreeForGem-1.7.6-tutorial-EN.mp4), [Русский](https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/FreeForGem-1.7.6-tutorial-RU.mp4) (subtitles: https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/FreeForGem-1.7.6-tutorial-EN.srt / -RU.srt).

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.6.tar.gz` | `1d282d61b88537090be2fd4e426ac99b8119bbb08a15ea50383cc7ac0a7916f6` |
| `freeforgem-1.7.6-linux-x86_64.tar.gz` | `f0cebb0ae3898384b3f659d5faa3ea59989547191ea3f1bc038cf726c182fb60` |
| `freeforgem-1.7.6-windows-x64.zip` | `ec7919dd663f7b224832488e8fadeedc222076983a02e56ae8b8749e133104f0` |

**Faster, safer auto-uv for all three algorithms (Pearl, NOID, CMFD).** One shared search and one profile store replace the
three copies; the state files (`uv.json`, `gpu-faults.json`, the restore markers) keep the 1.7.5 format, so profiles carry over
and a 1.7.6 file is read by 1.7.5.
* **30 / 15 MHz climb** (`--uv-coarse N`, default 30; `0` = one step size as before): the search climbs in 30 MHz steps and
  tries 15 MHz steps only inside the interval of the first failing step: the same edge in about half the steps. Pearl and CMFD
  had 15 MHz steps only. A driver error (Xid, hang) on a coarse step is not refined (no second crash): the result is one step below the
  last good step. The voltage-floor stop (power no longer drops) now also works for CMFD and scales with the step.
* **Better start points:** RTX 40 starts at +200 and RTX 50 at +300 in NOID and CMFD too; a new core lock starts 30 MHz below the offset
  of a verified profile of a higher lock (all three). The CMFD dry run prints the real start.
* **After a crash** the search stays 30 MHz below the crashed offset in all three (CMFD: 15 before); NOID remembers the last good step.
* **NOID and CMFD get what Pearl had:** periodic re-validation, a step back on a burst of rejected shares, detection of a tuned offset left on
  the card by a run that was cut short (it is no longer taken for the owner's offset), crash records and Xid attribution for CMFD.
* **Whose Xid:** an Xid that the kernel log pins on another process (another miner on the same rig) is not counted; a crash that came with another
  tool's change of the clocks / offset (HiveOS `nvidia-oc`) is not counted against the offset. A GPU that cannot be opened is waited for
  (1, 5, 15 min) instead of failing the offset.
* **Idle mode (`--idle`, Windows) for NOID and CMFD** (Pearl only in 1.7.5). CMFD has no light load (its engine pauses).
* **New: `--temp-limit C` / `--temp-stop C2`** (from ffgs) for laptops and mobile GPUs: the GPU works only the share of the time that keeps it below C.
  Pearl and NOID, NVIDIA. MANUAL.md, section 19.
* **CMFD no longer applies the HiveOS profile's memory lock** (e.g. Pearl's 810 MHz): on Rig01 it made CMFD ~6x slower (3.7 instead of ~21 FW/s). The core lock
  from the profile still applies; an explicit `--lock-mclock` still applies; the log says it is skipped.
* Not in this release: automatic core-lock choice (`--auto-lock` of ffgs), a fully shared tuning engine (planned).

## Русский

Файлы: `freeforgem-1.7.6.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/freeforgem-1.7.6.tar.gz`),
`freeforgem-1.7.6-linux-x86_64.tar.gz`, `freeforgem-1.7.6-windows-x64.zip`. Проверьте SHA-256 (MANUAL.md, раздел 9).

Видеоинструкция: [по-русски](https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/FreeForGem-1.7.6-tutorial-RU.mp4), [English](https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/FreeForGem-1.7.6-tutorial-EN.mp4) (субтитры: https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.6/FreeForGem-1.7.6-tutorial-RU.srt / -EN.srt).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.7.6.tar.gz` | `1d282d61b88537090be2fd4e426ac99b8119bbb08a15ea50383cc7ac0a7916f6` |
| `freeforgem-1.7.6-linux-x86_64.tar.gz` | `f0cebb0ae3898384b3f659d5faa3ea59989547191ea3f1bc038cf726c182fb60` |
| `freeforgem-1.7.6-windows-x64.zip` | `ec7919dd663f7b224832488e8fadeedc222076983a02e56ae8b8749e133104f0` |

**Быстрее и безопаснее auto-uv для всех трёх алгоритмов (Pearl, NOID, CMFD).** Один общий подбор и одно хранилище профилей вместо трёх копий;
файлы состояния (`uv.json`, `gpu-faults.json`, маркеры восстановления) остаются в формате 1.7.5: профили переносятся, файл 1.7.6 читает 1.7.5.
* **Подъём 30 / 15 МГц** (`--uv-coarse N`, по умолчанию 30; `0` — один размер шага, как раньше): подбор идёт шагами 30 МГц и пробует 15 МГц только внутри
  интервала первого неудачного шага: тот же край примерно за вдвое меньшее число шагов. У Pearl и CMFD было только 15 МГц. Сбой драйвера (Xid, зависание)
  на крупном шаге не уточняется (второго падения нет): результат — на шаг ниже последнего удачного. Останов по полу напряжения (мощность перестала падать)
  теперь работает и для CMFD и масштабируется шагом.
* **Лучше старт:** RTX 40 стартует с +200, RTX 50 с +300 и в NOID, и в CMFD; новый лок ядра стартует на 30 МГц ниже offset проверенного профиля более высокого лока (все три).
  Сухой прогон CMFD печатает настоящий старт.
* **После падения** поиск остаётся на 30 МГц ниже упавшего offset во всех трёх (у CMFD было 15); NOID помнит последний удачный шаг.
* **NOID и CMFD получают то, что было у Pearl:** периодическую перепроверку, откат на шаг при всплеске отклонённых шаров, обнаружение подобранного offset, оставленного на карте
  оборванным запуском (его больше не принимают за offset владельца), записи падений и атрибуцию Xid для CMFD.
* **Чей Xid:** Xid, который журнал ядра приписывает другому процессу (другой майнер на том же риге), не засчитывается; падение, совпавшее со сменой частот / offset другим
  инструментом (HiveOS `nvidia-oc`), не засчитывается против offset. GPU, которую нельзя открыть, ждут (1, 5, 15 мин) вместо отказа offset.
* **Режим простоя (`--idle`, Windows) для NOID и CMFD** (в 1.7.5 только Pearl). У CMFD нет лёгкой нагрузки (движок встаёт на паузу).
* **Новое: `--temp-limit C` / `--temp-stop C2`** (из ffgs) для ноутбуков и мобильных GPU: карта работает лишь ту долю времени, при которой держится ниже C.
  Pearl и NOID, NVIDIA. MANUAL.md, раздел 19.
* **CMFD больше не применяет лок памяти из профиля HiveOS** (например 810 МГц для Pearl): на Rig01 он замедлял CMFD в ~6 раз (3,7 вместо ~21 FW/s). Лок ядра
  из профиля применяется по-прежнему; явный `--lock-mclock` действует; в лог пишется, что лок пропущен.
* Не вошло: автовыбор лока ядра (`--auto-lock` из ffgs), полностью общий движок подбора (запланирован).
