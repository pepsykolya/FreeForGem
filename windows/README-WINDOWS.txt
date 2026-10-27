FreeForGem for Windows 10/11 (x64) - Pearl (PRL), Common Foundry (CMFD) and NOID GPU miner for NVIDIA RTX 20/30/40/50 and AMD Radeon RX 6000+
Full manual (EN/RU/ZH): MANUAL.md  (https://github.com/pepsykolya/FreeForGem/blob/main/MANUAL.md)

QUICK START
1. Unpack this archive to a folder without spaces or non-English letters, e.g. C:\FreeForGem
2. Pick the start file for your pool (all are ready to run):
     start.bat / start-luckypool-eu.bat   LuckyPool Europe   pearl-eu2.luckypool.io:3360
     start-luckypool-ru / -us / -asia     LuckyPool Russia / US East / Singapore
     start-kryptex-eu.bat                 Kryptex Europe     prl-eu.kryptex.network:7048
     start-kryptex-ru / -us / -asia       Kryptex Russia / North America / Singapore
     start-nvidia-smi.bat                 advanced, as Administrator: nvidia-smi clock lock + power limit (MANUAL.md, section 5)
     start-cmfd.bat                       Common Foundry (CMFD), pool Aria (Midas Mining and others inside the file);
                                          WALLET = your CMFD address (cmfd1... or 64 hex); ~7 GB free GPU memory; the 6.4 GB model is
                                          downloaded once into cmfd-model (MANUAL.md, section 13). Run as Administrator if
                                          the card has an undervolt offset: the miner sets it to 0 for CMFD and puts it back.
     start-noid.bat                       NOID (Parano1d), pool InnovLab; WALLET = your o1... address; NVIDIA RTX 30/40/50 and driver 580+ (MANUAL.md, section 15)
   Right-click it -> Edit and REPLACE WALLET with your own prl1... address: it is preset to the developer's
   example address, and with it you would mine for the developer. Optionally set WORKER and DEVICES. Save.
   LuckyPool also pays NOCK for the same work (merge mining): set WALLET=prl1...+YOUR_NOCK_ADDRESS (LuckyPool files only).
3. Double-click the chosen .bat. Logs: logs\miner-<date>.log. Ctrl+C stops the miner.
   A miner that exits or crashes is restarted automatically after 15 s.

Needs only the NVIDIA driver 525 or newer (no CUDA, no Python, no WSL). One process drives all GPUs.
AMD Radeon RX 6000 (RDNA2, tested on RX 6800 XT; since 1.6.0): needs only the AMD Adrenalin driver, the same start files work
(the miner picks the AMD library when no NVIDIA GPU is found; integrated Radeon graphics are skipped). AUTO_UV=1 on AMD runs
amduv.exe (tunes once ~10-15 min, restores your settings on exit; run as Administrator). MANUAL.md, section 14.
Self-test (optional): open cmd in this folder and run  freeforgem.exe --self-test
If the log says "embedded search kernel could not be loaded", update the NVIDIA driver (525+); the GPU must be Ampere (RTX 30xx) or newer.
If Defender/SmartScreen blocks the start: "More info -> Run anyway", or add the folder to the exclusions.
Check the archive SHA-256 against the release page:  certutil -hashfile freeforgem-1.7.3-windows-x64.zip SHA256

POOLS (POOL=HOST:PORT; ports 3360 / 3361 / 3362 = share difficulty 2M / 4M / 8M)
pearl-eu2.luckypool.io:3360 (default, Europe); Kryptex prl-eu.kryptex.network:7048; see MANUAL.md for the other regions.
Proofs are sent gzip-compressed (~5 KB per share instead of ~270 KB).

OPTIONAL UNDERVOLT: in start.bat remove "rem" before  set "AUTO_UV=1"  and run start.bat as Administrator (step by step: MANUAL.md, section 12, "Windows: step by step").
The miner then looks for a lower, safe GPU voltage (saves watts at the same hashrate) and restores your settings on exit. See MANUAL.md, section 12.
The tuner tries offsets up to +495 MHz (RTX 20xx/GTX 16xx/CMP 50HX: +225, CMP 30HX/40HX: +150); to change that set UV_MAX in start.bat
(e.g. set "UV_MAX=480", at most 600; a mixed rig can give one value per GPU in DEVICES order: set "UV_MAX=225,495,150").
Since 1.7.2 the result is kept per core clock lock (LOCK_CCLOCK); --uv-reset in EXTRA forgets the current lock only, --uv-reset-all forgets all.
Results of a FreeForGemPear folder next to this one are imported on the first start.

DEV FEE 1 %: one 60 s slot in every 100 min on the same pool (worker ffgp-devfee; on LuckyPool also NOCK). See LICENSE.

------------------------------------------------------------------------------------------------
FreeForGem для Windows 10/11 (x64)

БЫСТРЫЙ СТАРТ
1. Распакуйте архив в папку без пробелов и русских букв, например C:\FreeForGem
2. Выберите файл запуска под свой пул (все готовы к работе):
     start.bat / start-luckypool-eu.bat   LuckyPool Европа   pearl-eu2.luckypool.io:3360
     start-luckypool-ru / -us / -asia     LuckyPool Россия / США (восток) / Сингапур
     start-kryptex-eu.bat                 Kryptex Европа     prl-eu.kryptex.network:7048
     start-kryptex-ru / -us / -asia       Kryptex Россия / Северная Америка / Сингапур
     start-nvidia-smi.bat                 для опытных, от администратора: nvidia-smi частоты + лимит мощности (MANUAL.md, раздел 5)
     start-cmfd.bat                       Common Foundry (CMFD), пул Aria (Midas Mining и другие - внутри файла);
                                          WALLET = ваш адрес CMFD (cmfd1... или 64 hex); ~7 ГБ свободной памяти GPU; модель 6.4 ГБ
                                          скачивается один раз в cmfd-model (MANUAL.md, раздел 13). Запускайте от администратора,
                                          если на карте стоит андервольт-offset: майнер ставит его в 0 для CMFD и возвращает обратно.
     start-noid.bat                       NOID (Parano1d), пул InnovLab; WALLET = ваш адрес o1...; NVIDIA RTX 30/40/50 и драйвер 580+ (MANUAL.md, раздел 15)
   Правой кнопкой -> Изменить и ЗАМЕНИТЕ WALLET своим адресом prl1...: там заранее стоит адрес разработчика
   из примера, и с ним вы майните на разработчика. При желании задайте WORKER и DEVICES. Сохраните.
   LuckyPool за ту же работу платит ещё и NOCK (merge-майнинг): WALLET=prl1...+ВАШ_АДРЕС_NOCK (только батники LuckyPool).
3. Запустите выбранный .bat. Логи: logs\miner-<дата>.log. Ctrl+C - остановка.
   Если майнер завершился или упал, он автоматически перезапускается через 15 с.

Нужен только драйвер NVIDIA 525 или новее (CUDA, Python и WSL не нужны). Один процесс работает со всеми картами.
AMD Radeon RX 6000 (RDNA2, проверено на RX 6800 XT; с 1.6.0): нужен только драйвер AMD Adrenalin, подходят те же файлы запуска
(майнер сам берёт AMD-библиотеку, если карт NVIDIA нет; встроенная графика Radeon пропускается). AUTO_UV=1 на AMD запускает
amduv.exe (подбор один раз ~10-15 мин, при выходе возвращает ваши настройки; запуск от администратора). MANUAL.md, раздел 14.
Проверка (по желанию): в cmd в этой папке выполните  freeforgem.exe --self-test
Если Defender/SmartScreen блокирует запуск: "Подробнее -> Выполнить в любом случае" или добавьте папку в исключения.
Сверьте SHA-256 архива со страницей релиза:  certutil -hashfile freeforgem-1.7.3-windows-x64.zip SHA256

АНДЕРВОЛЬТ (по желанию): в start.bat уберите "rem" перед  set "AUTO_UV=1"  и запустите start.bat от администратора.
С 1.7.2 результат хранится для каждого лока ядра (LOCK_CCLOCK); --uv-reset в EXTRA забывает только текущий лок, --uv-reset-all - все.
Полное описание: MANUAL.md (раздел 12 - андервольт); пулы - раздел 6, комиссия 1 % - раздел 10.

------------------------------------------------------------------------------------------------
FreeForGem Windows 10/11 (x64) 版

快速开始
1. 将压缩包解压到不含空格和中文字符的文件夹，例如 C:\FreeForGem
2. 选择对应矿池的启动文件（均可直接运行）：
     start.bat / start-luckypool-eu.bat   LuckyPool 欧洲   pearl-eu2.luckypool.io:3360
     start-luckypool-ru / -us / -asia     LuckyPool 俄罗斯 / 美国东部 / 新加坡
     start-kryptex-eu.bat                 Kryptex 欧洲     prl-eu.kryptex.network:7048
     start-kryptex-ru / -us / -asia       Kryptex 俄罗斯 / 北美 / 新加坡
     start-nvidia-smi.bat                 进阶，需管理员：nvidia-smi 锁频 + 功耗限制（MANUAL.md 第 5 节）
     start-cmfd.bat                       Common Foundry (CMFD)，矿池 Aria（Midas Mining 等见文件内）；
                                          WALLET = 你的 CMFD 地址（cmfd1... 或 64 位十六进制）；约 7 GB 空闲显存；6.4 GB 模型
                                          只下载一次到 cmfd-model（MANUAL.md 第 13 节）。如果显卡设置了降压偏移，请以管理员身份运行：
                                          矿工在 CMFD 期间将其设为 0，退出时恢复。
     start-noid.bat                       NOID (Parano1d)，矿池 InnovLab；WALLET = 你的 o1... 地址；NVIDIA RTX 30/40/50，驱动 580+（MANUAL.md 第 15 节）
   右键 -> 编辑，把 WALLET 替换为你自己的 prl1... 地址：默认是开发者的示例地址，不替换就是在为开发者挖矿。
   可按需设置 WORKER 和 DEVICES。保存。
   LuckyPool 还会为同样的算力支付 NOCK（合并挖矿）：WALLET=prl1...+你的NOCK地址（仅 LuckyPool 启动文件）。
3. 运行所选的 .bat。日志：logs\miner-<日期>.log。Ctrl+C 停止。矿工退出或崩溃后会在 15 秒后自动重启。

只需要 NVIDIA 驱动 525 或更新版本（不需要 CUDA、Python 和 WSL）。一个进程驱动所有显卡。
如果 Defender/SmartScreen 阻止运行：点击"更多信息 -> 仍要运行"，或将该文件夹加入排除项。
请对照发布页面核对压缩包的 SHA-256：  certutil -hashfile freeforgem-1.7.3-windows-x64.zip SHA256

可选降压：在 start.bat 中删除  set "AUTO_UV=1"  前面的 "rem"，并以管理员身份运行 start.bat。
自 1.7.2 起结果按核心锁频（LOCK_CCLOCK）分别保存；在 EXTRA 中加 --uv-reset 只清除当前锁频，--uv-reset-all 清除全部。
完整说明见 MANUAL.md（第 12 节：降压）；矿池见第 6 节，1% 开发者费用见第 10 节。
