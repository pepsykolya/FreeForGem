# FreeForGem 1.7.7 — release notes

[English](#english) · [Русский](#русский)

## English

Files: `freeforgem-1.7.7_hiveos.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.7/freeforgem-1.7.7_hiveos.tar.gz`),
`freeforgem-1.7.7-linux-x86_64.tar.gz`, `freeforgem-1.7.7-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.7_hiveos.tar.gz` | `b61356d568428290c0060e6c8a169a820f970a9c9a5d2ae41da6b322cad6ebcd` |
| `freeforgem-1.7.7-linux-x86_64.tar.gz` | `fd0697eec486add58fc3f9b7d2cefd2be71ef70f103692cf0d414ce89c701c31` |
| `freeforgem-1.7.7-windows-x64.zip` | `0ea283ec8c9b5406ecfed86637e6f60d0cb0170c6fac041ff93d82b769193f68` |

**Small efficiency gains and two new helpers.**
* **NOID: about +0.6 % MH/W** (RTX 30 and RTX 40, `--noid-mode eff`): a new eff kernel (`t512vs`) for all supported architectures, same hashrate
  within 0.15 %, about 0.7 % less power. The saved auto-uv profiles of NOID are re-checked once (the kernel name is part of the profile);
  `NOID_KERNEL=t512va` brings the old kernel back.
* **Pearl, RTX 30: about +0.3 % hashrate at the same power.** The RTX 30 kernel works on a larger jackpot tile (16 x 16 instead of 8 x 16). With
  `--target-mode jackpot` (the pool sends a threshold made for the smaller tile) the miner runs the previous 8 x 16 kernel, also when the mode
  changes while mining. Ada, Turing and RTX 50 are unchanged. The profiles of auto-uv for RTX 30 are re-checked once.
* **Pearl, RTX 30: a hint in the log** to lock the core at 1530 MHz (`--lock-cclock 1530`): about 3.6 % better TH/W for about 2 % less hashrate on an
  RTX 3070. The miner only prints the hint; it sets nothing. A new lock gets its own auto-uv search.
* **New: `--cmp-driver`** (HiveOS, off by default): installs the unlocked NVIDIA driver build for CMP cards. In the flight sheet add the word
  `cmp-unlock-driver` to *Extra config arguments*. It replaces the kernel driver of the whole rig and reboots it: read MANUAL.md, section 20, first.
* **HiveOS:** when a newer release is out, the dashboard shows `need update!!` next to the version (`1.7.7 0.652 TH/W, need update!!`).
* **CMP 50HX: the auto-uv limit is raised from +225 to +330** (`--uv-max` still overrides it; RTX 20 / GTX 16 stay at +225). A saved result that stopped at the old limit (no failing step, not the voltage floor) is searched further up automatically from that offset at the next start; Pearl, NOID and CMFD.
* **Memory clock lock check:** the miner now warns (yellow, once per card) when a memory clock lock does not hold under load. Example: `--lock-mclock 405` on an RTX 3070 is accepted by the driver, but the card computes at 810 MHz; the miner used to say only "locked to 405".
* **auto-uv:** a repeated search now starts one coarse step below the last result of the card instead of from the bottom; the "voltage floor" verdict is given only at a comparable card temperature (a card that is still warming up is no longer reported as having hit the floor); on a rig with little free RAM, a check that could not start is planned again once the miner has settled (the check plan keeps 128 MB spare instead of 256).
* **Windows:** the console window no longer freezes the miner (QuickEdit is switched off, so a click in the window does not stop it), the output goes through a bounded queue with its own writer thread, and the statistics line is no longer painted red after one rejected share (only real events are coloured).
* Pearl on Ada (RTX 40) uses the same kernel as 1.7.6 and was not re-tested on hardware in this release.

## Русский

Файлы: `freeforgem-1.7.7_hiveos.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.7/freeforgem-1.7.7_hiveos.tar.gz`),
`freeforgem-1.7.7-linux-x86_64.tar.gz`, `freeforgem-1.7.7-windows-x64.zip`. Проверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.7.7_hiveos.tar.gz` | `b61356d568428290c0060e6c8a169a820f970a9c9a5d2ae41da6b322cad6ebcd` |
| `freeforgem-1.7.7-linux-x86_64.tar.gz` | `fd0697eec486add58fc3f9b7d2cefd2be71ef70f103692cf0d414ce89c701c31` |
| `freeforgem-1.7.7-windows-x64.zip` | `0ea283ec8c9b5406ecfed86637e6f60d0cb0170c6fac041ff93d82b769193f68` |

**Небольшие выигрыши в эффективности и два новых помощника.**
* **NOID: около +0,6 % МН/Вт** (RTX 30 и RTX 40, `--noid-mode eff`): новое eff-ядро (`t512vs`) для всех поддерживаемых архитектур, хэшрейт тот же
  (в пределах 0,15 %), мощность примерно на 0,7 % ниже. Сохранённые профили auto-uv для NOID перепроверятся один раз (имя ядра входит в профиль);
  `NOID_KERNEL=t512va` возвращает старое ядро.
* **Pearl, RTX 30: около +0,3 % хэшрейта при той же мощности.** Ядро RTX 30 работает на большем jackpot-тайле (16 x 16 вместо 8 x 16). При
  `--target-mode jackpot` (пул присылает порог под меньший тайл) майнер запускает прежнее ядро 8 x 16, в том числе когда режим меняется на лету.
  Ada, Turing и RTX 50 без изменений. Профили auto-uv для RTX 30 перепроверятся один раз.
* **Pearl, RTX 30: подсказка в логе** про лок ядра 1530 МГц (`--lock-cclock 1530`): около 3,6 % лучше TH/Вт при примерно 2 % меньшем хэшрейте на
  RTX 3070. Майнер только пишет подсказку и ничего не ставит. Для нового лока auto-uv ищет свой профиль.
* **Новое: `--cmp-driver`** (HiveOS, по умолчанию выключено): ставит разблокированную сборку драйвера NVIDIA для карт CMP. В полётном листе добавьте
  слово `cmp-unlock-driver` в *Extra config arguments*. Заменяется драйвер ядра на всём риге и рига перезагружается: сначала прочитайте MANUAL.md, раздел 20.
* **HiveOS:** когда вышла новая версия, на панели рядом с версией пишется `need update!!` (`1.7.7 0.652 TH/W, need update!!`).
* **CMP 50HX: предел auto-uv поднят с +225 до +330** (`--uv-max` по-прежнему главнее; RTX 20 / GTX 16 остаются на +225). Сохранённый результат, упёршийся в старый предел (сбойного шага не было, это не voltage floor), при следующем старте автоматически дотюнивается вверх от этого offset; Pearl, NOID и CMFD.
* **Проверка лока памяти:** майнер теперь предупреждает (жёлтым, один раз на карту), если лок частоты памяти не держится под нагрузкой. Пример: `--lock-mclock 405` на RTX 3070 драйвер принимает, но карта считает на 810 МГц; раньше майнер писал только «locked to 405».
* **auto-uv:** повторный подбор теперь стартует на один крупный шаг ниже прошлого результата карты, а не с нижней границы; вердикт «voltage floor» выносится только при сопоставимой температуре карты (прогревающаяся карта больше не считается упёршейся в пол); на риге с малым запасом RAM проверка, которая не смогла стартовать, планируется заново после прогрева майнера (план проверок оставляет запас 128 МБ вместо 256).
* **Windows:** окно консоли больше не замораживает майнер (QuickEdit выключен, клик в окне его не останавливает), вывод идёт через ограниченную очередь с отдельным потоком записи, строка статистики не краснеет после одного отклонённого шара (красятся только события).
* Pearl на Ada (RTX 40) работает тем же ядром, что в 1.7.6, и в этом релизе на железе повторно не проверялся.
