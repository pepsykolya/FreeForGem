FreeForGemPear for Windows 10/11 (x64) - Pearl (PRL) GPU miner for NVIDIA RTX 30xx (RTX 40xx should work, untuned)
Full manual (EN/RU/ZH): MANUAL.md  (https://github.com/pepsykolya/FreeForGemPear/blob/main/MANUAL.md)

QUICK START
1. Unpack this archive to a folder without spaces or non-English letters, e.g. C:\FreeForGemPear
2. Right-click start.bat -> Edit. Set WALLET (your prl1... address) and optionally POOL, WORKER, DEVICES. Save.
3. Double-click start.bat. Logs: logs\miner-<date>.log. Ctrl+C stops the miner.
   A miner that exits or crashes is restarted automatically after 15 s.

Needs only the NVIDIA driver 525 or newer (no CUDA, no Python, no WSL). One process drives all GPUs.
Self-test (optional): open cmd in this folder and run  freeforgempear.exe --self-test
If the log says "embedded search kernel could not be loaded", update the NVIDIA driver (525+); the GPU must be Ampere (RTX 30xx) or newer.
If Defender/SmartScreen blocks the start: "More info -> Run anyway", or add the folder to the exclusions.
Check the archive SHA-256 against the release page:  certutil -hashfile freeforgempear-1.3.1-windows-x64.zip SHA256

POOLS (POOL=HOST:PORT; ports 3360 / 3361 / 3362 = share difficulty 2M / 4M / 8M)
pearl-eu2.luckypool.io:3360 (default, Europe), see MANUAL.md for the other regions.

DEV FEE 1 %: one 60 s slot in every 100 min on the same pool (worker ffgp-devfee). See LICENSE.

------------------------------------------------------------------------------------------------
FreeForGemPear для Windows 10/11 (x64)

БЫСТРЫЙ СТАРТ
1. Распакуйте архив в папку без пробелов и русских букв, например C:\FreeForGemPear
2. Правой кнопкой по start.bat -> Изменить. Задайте WALLET (адрес prl1...) и при желании POOL, WORKER, DEVICES. Сохраните.
3. Запустите start.bat. Логи: logs\miner-<дата>.log. Ctrl+C - остановка.
   Если майнер завершился или упал, он автоматически перезапускается через 15 с.

Нужен только драйвер NVIDIA 525 или новее (CUDA, Python и WSL не нужны). Один процесс работает со всеми картами.
Проверка (по желанию): в cmd в этой папке выполните  freeforgempear.exe --self-test
Если Defender/SmartScreen блокирует запуск: "Подробнее -> Выполнить в любом случае" или добавьте папку в исключения.
Сверьте SHA-256 архива со страницей релиза:  certutil -hashfile freeforgempear-1.3.1-windows-x64.zip SHA256

------------------------------------------------------------------------------------------------
FreeForGemPear Windows 10/11 (x64) 版

快速开始
1. 将压缩包解压到不含空格和中文字符的文件夹，例如 C:\FreeForGemPear
2. 右键点击 start.bat -> 编辑。设置 WALLET（你的 prl1... 地址），也可按需设置 POOL、WORKER、DEVICES。保存。
3. 运行 start.bat。日志：logs\miner-<日期>.log。Ctrl+C 停止。矿工退出或崩溃后会在 15 秒后自动重启。

只需要 NVIDIA 驱动 525 或更新版本（不需要 CUDA、Python 和 WSL）。一个进程驱动所有显卡。
如果 Defender/SmartScreen 阻止运行：点击"更多信息 -> 仍要运行"，或将该文件夹加入排除项。
