FreeForGemPear for Windows 10/11 (x64) - Pearl (PRL) GPU miner for NVIDIA RTX 30xx (RTX 40xx should work, untuned)
Full manual (EN/RU/ZH): MANUAL.md  (https://github.com/pepsykolya/FreeForGemPear/blob/main/MANUAL.md)

QUICK START
1. Unpack this archive to a folder without spaces or non-English letters, e.g. C:\FreeForGemPear
2. Pick the start file for your pool (all are ready to run):
     start.bat / start-luckypool-eu.bat   LuckyPool Europe   pearl-eu2.luckypool.io:3360
     start-luckypool-ru / -us / -asia     LuckyPool Russia / US East / Singapore
     start-kryptex-eu.bat                 Kryptex Europe     prl-eu.kryptex.network:7048
     start-kryptex-ru / -us / -asia       Kryptex Russia / North America / Singapore
     start-nvidia-smi.bat                 advanced, as Administrator: nvidia-smi clock lock + power limit (MANUAL.md, section 5)
   Right-click it -> Edit and REPLACE WALLET with your own prl1... address: it is preset to the developer's
   example address, and with it you would mine for the developer. Optionally set WORKER and DEVICES. Save.
   LuckyPool also pays NOCK for the same work (merge mining): set WALLET=prl1...+YOUR_NOCK_ADDRESS (LuckyPool files only).
3. Double-click the chosen .bat. Logs: logs\miner-<date>.log. Ctrl+C stops the miner.
   A miner that exits or crashes is restarted automatically after 15 s.

Needs only the NVIDIA driver 525 or newer (no CUDA, no Python, no WSL). One process drives all GPUs.
Self-test (optional): open cmd in this folder and run  freeforgempear.exe --self-test
If the log says "embedded search kernel could not be loaded", update the NVIDIA driver (525+); the GPU must be Ampere (RTX 30xx) or newer.
If Defender/SmartScreen blocks the start: "More info -> Run anyway", or add the folder to the exclusions.
Check the archive SHA-256 against the release page:  certutil -hashfile freeforgempear-1.5.1-windows-x64.zip SHA256

POOLS (POOL=HOST:PORT; ports 3360 / 3361 / 3362 = share difficulty 2M / 4M / 8M)
pearl-eu2.luckypool.io:3360 (default, Europe); Kryptex prl-eu.kryptex.network:7048; see MANUAL.md for the other regions.
Proofs are sent gzip-compressed (~5 KB per share instead of ~270 KB).

OPTIONAL UNDERVOLT: in start.bat remove "rem" before  set "AUTO_UV=1"  and run start.bat as Administrator (step by step: MANUAL.md, section 12, "Windows: step by step").
The miner then looks for a lower, safe GPU voltage (saves watts at the same hashrate) and restores your settings on exit. See MANUAL.md, section 12.
The tuner tries offsets up to +495 MHz; to change that set UV_MAX in start.bat (e.g. set "UV_MAX=480", at most 600).

DEV FEE 1 %: one 60 s slot in every 100 min on the same pool (worker ffgp-devfee; on LuckyPool also NOCK). See LICENSE.

------------------------------------------------------------------------------------------------
FreeForGemPear для Windows 10/11 (x64)

БЫСТРЫЙ СТАРТ
1. Распакуйте архив в папку без пробелов и русских букв, например C:\FreeForGemPear
2. Выберите файл запуска под свой пул (все готовы к работе):
     start.bat / start-luckypool-eu.bat   LuckyPool Европа   pearl-eu2.luckypool.io:3360
     start-luckypool-ru / -us / -asia     LuckyPool Россия / США (восток) / Сингапур
     start-kryptex-eu.bat                 Kryptex Европа     prl-eu.kryptex.network:7048
     start-kryptex-ru / -us / -asia       Kryptex Россия / Северная Америка / Сингапур
     start-nvidia-smi.bat                 для опытных, от администратора: nvidia-smi частоты + лимит мощности (MANUAL.md, раздел 5)
   Правой кнопкой -> Изменить и ЗАМЕНИТЕ WALLET своим адресом prl1...: там заранее стоит адрес разработчика
   из примера, и с ним вы майните на разработчика. При желании задайте WORKER и DEVICES. Сохраните.
   LuckyPool за ту же работу платит ещё и NOCK (merge-майнинг): WALLET=prl1...+ВАШ_АДРЕС_NOCK (только батники LuckyPool).
3. Запустите выбранный .bat. Логи: logs\miner-<дата>.log. Ctrl+C - остановка.
   Если майнер завершился или упал, он автоматически перезапускается через 15 с.

Нужен только драйвер NVIDIA 525 или новее (CUDA, Python и WSL не нужны). Один процесс работает со всеми картами.
Проверка (по желанию): в cmd в этой папке выполните  freeforgempear.exe --self-test
Если Defender/SmartScreen блокирует запуск: "Подробнее -> Выполнить в любом случае" или добавьте папку в исключения.
Сверьте SHA-256 архива со страницей релиза:  certutil -hashfile freeforgempear-1.5.1-windows-x64.zip SHA256

------------------------------------------------------------------------------------------------
FreeForGemPear Windows 10/11 (x64) 版

快速开始
1. 将压缩包解压到不含空格和中文字符的文件夹，例如 C:\FreeForGemPear
2. 选择对应矿池的启动文件（均可直接运行）：
     start.bat / start-luckypool-eu.bat   LuckyPool 欧洲   pearl-eu2.luckypool.io:3360
     start-luckypool-ru / -us / -asia     LuckyPool 俄罗斯 / 美国东部 / 新加坡
     start-kryptex-eu.bat                 Kryptex 欧洲     prl-eu.kryptex.network:7048
     start-kryptex-ru / -us / -asia       Kryptex 俄罗斯 / 北美 / 新加坡
     start-nvidia-smi.bat                 进阶，需管理员：nvidia-smi 锁频 + 功耗限制（MANUAL.md 第 5 节）
   右键 -> 编辑，把 WALLET 替换为你自己的 prl1... 地址：默认是开发者的示例地址，不替换就是在为开发者挖矿。
   可按需设置 WORKER 和 DEVICES。保存。
   LuckyPool 还会为同样的算力支付 NOCK（合并挖矿）：WALLET=prl1...+你的NOCK地址（仅 LuckyPool 启动文件）。
3. 运行所选的 .bat。日志：logs\miner-<日期>.log。Ctrl+C 停止。矿工退出或崩溃后会在 15 秒后自动重启。

只需要 NVIDIA 驱动 525 或更新版本（不需要 CUDA、Python 和 WSL）。一个进程驱动所有显卡。
如果 Defender/SmartScreen 阻止运行：点击"更多信息 -> 仍要运行"，或将该文件夹加入排除项。
