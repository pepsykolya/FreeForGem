<!-- FreeForGemPear — see LICENSE -->
# FreeForGemPear — Manual

* [English](#english)
* [Русский](#русский)
* [中文](#中文)

---

## English

### 1. What it is

FreeForGemPear is a GPU miner for **Pearl (PRL)** (pearlhash proof-of-useful-work) on NVIDIA GPUs, with a **1% developer fee**.
It has its own CUDA kernel for Ampere: the A′ matrix is generated on-chip, the int8 tensor-core GEMM runs in a persistent
kernel, and the jackpot fold and the BLAKE3 hash are fused into it. The main loop is hand-tuned at SASS level.
The host builds and checks every share proof with the official `zk-pow` verifier before it is submitted.

| GPU series | Status |
|---|---|
| RTX 30xx (Ampere, sm_86): 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **main target, tuned: ~70 TH/s on an RTX 3070 at ~107 W** |
| RTX 40xx (Ada, sm_89) | runs the sm_86 code, untuned |
| Older GPUs, A100 (sm_80), AMD | not supported |

### 2. Requirements

* NVIDIA GPU of the RTX 30 series (RTX 40 runs untuned) and NVIDIA driver **525 or newer**.
* HiveOS, Linux x86_64 (glibc 2.31+, Ubuntu 20.04+), or Windows 10/11 x64. No CUDA toolkit, no Python, no WSL.
* A PRL wallet address (`prl1...`) and an internet connection to a Pearl stratum pool.
* Video memory: the kernel works from the L2 cache and needs little VRAM.

### 3. HiveOS setup (step by step)

FreeForGemPear runs on HiveOS as a **Custom miner**: no SSH and no manual installation.

**Installation URL (1.3.1):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.1/freeforgempear-1.3.1.tar.gz
```

1. **Wallet.** In *Wallets*, add your PRL wallet: coin `PRL`, your `prl1...` address.
2. **New flight sheet.**
   * **Coin:** `PRL`.
   * **Wallet:** the wallet from step 1.
   * **Pool:** *Configure in miner*.
   * **Miner:** *Custom*, then press *Setup Miner Config*.
3. **Miner config:**

   | Field | Value |
   |---|---|
   | Miner name | `freeforgempear` (filled in automatically after you paste the URL) |
   | Installation URL | the link above |
   | Hash algorithm | `pearlhash` (or leave `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360` (or any pool from section 6) |
   | Pass | leave empty |
   | Extra config arguments | optional, for example `--devices 0,1` |

4. **Apply.** *Apply changes*, *Create flight sheet*, then apply it to the rig.

The miner log is at `/var/log/miner/freeforgempear/freeforgempear.log` and in the HiveOS miner console.
The dashboard shows the hashrate per GPU and in total, temperatures, fans, and accepted and rejected shares.
To update, change the Installation URL to the new version. The same guide with more details: [HIVEOS.md](HIVEOS.md).

### 4. Linux without HiveOS

```
tar xzf freeforgempear-1.3.1-linux-x86_64.tar.gz
cd freeforgempear-1.3.1
./freeforgempear --self-test        # optional: checks the GPU kernel against the CPU reference
./freeforgempear --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1
```

| Option | Meaning |
|---|---|
| `--pool HOST:PORT` | stratum pool (default `pearl-eu2.luckypool.io:3360`) |
| `--wallet ADDRESS` | your PRL address (required) |
| `--worker NAME` | worker name (default `worker1`) |
| `--devices 0,1,...` | GPUs to use (default: all visible; honours `CUDA_VISIBLE_DEVICES`) |
| `--minutes N` | stop after N minutes (default: run until Ctrl-C / SIGTERM) |
| `--self-test` | verify the kernel and exit |

One process drives all GPUs and reconnects to the pool on its own. For a permanent setup run it under `systemd`, `screen` or `tmux`.

### 5. Windows 10/11

Native Windows build: no WSL, no CUDA installation, no Python. Only the NVIDIA driver is needed.

1. Download **`freeforgempear-1.3.1-windows-x64.zip`** from the release page:
   `https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.1/freeforgempear-1.3.1-windows-x64.zip`
   and check its SHA-256 (section 9).
2. Unpack it to a folder **without spaces or non-English letters**, e.g. `C:\FreeForGemPear`.
3. Right-click **`start.bat`** → *Edit* and set:

| Setting | Value |
|---|---|
| `WALLET` | your PRL address (`prl1...`) |
| `POOL` | `HOST:PORT`, default `pearl-eu2.luckypool.io:3360` (section 6) |
| `WORKER` | rig name, default = computer name |
| `DEVICES` | empty = all NVIDIA GPUs, or a list like `0,1` (indexes from `nvidia-smi`) |

4. Double-click **`start.bat`**. One miner process drives all GPUs; its output is shown in the window and saved to
   `logs\miner-<date>.log` (the last 10 logs are kept). If the miner exits or crashes, it is restarted after 15 s.
   `Ctrl+C` stops it.

* **Autostart:** put a shortcut to `start.bat` into `shell:startup` (Win+R, `shell:startup`), or create a Task Scheduler
  task "At log on" that runs `start.bat`.
* **Overclocking:** MSI Afterburner (core lock in the curve editor, memory down) with the values from section 7.
  Laptops: plug in the charger and choose the maximum performance mode.
* Defender / SmartScreen may flag a new unsigned `.exe`: *More info → Run anyway*, or add the folder to the exclusions.
* Without `start.bat`: `freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet YOUR_PRL_ADDRESS --worker rig1`
* Build it yourself: `.\build-windows.ps1` (see the README).

### 6. Pools

Default: **LuckyPool**, `pearl-eu2.luckypool.io:3360` (plain TCP stratum). The hosts and ports below are taken from the pool's own
configuration (`https://pearl.luckypool.io/api/stats`, `config.stratum` and `config.ports`); pick the region closest to you:

| Region | Host |
|---|---|
| Europe | `pearl-eu2.luckypool.io` (default), `pearl-eu1.luckypool.io`, `pearl-pl.luckypool.io` (Poland), `pearl-tr.luckypool.io` (Turkey) |
| Russia | `pearl-ru.luckypool.io` |
| US | `pearl-us-east.luckypool.io`, `pearl-us-west.luckypool.io`, `pearl-us-central.luckypool.io`, `pearl-us-ord.luckypool.io` |
| South America | `pearl-br.luckypool.io` (Brazil) |
| Canada | `pearl-ca1.luckypool.io`, `pearl-ca2.luckypool.io` |
| Asia | `pearl-sg1.luckypool.io`, `pearl-sg2.luckypool.io` (Singapore), `pearl-id.luckypool.io` (Indonesia), `pearl-hk.luckypool.io` (Hong Kong), `pearl-in.luckypool.io` (India), `pearl-jp.luckypool.io` (Japan), `pearl-au.luckypool.io` (Australia) |

| Port | Share difficulty (vardiff) |
|---|---|
| `3360` | 2 000 000 (default) |
| `3361` | 4 000 000 |
| `3362` | 8 000 000 |

Example: `--pool pearl-pl.luckypool.io:3360`. Use your plain `prl1...` address as the wallet; the worker name is sent separately.
Any other Pearl stratum pool that speaks the same JSON-RPC stratum can be used; the dev fee (section 10) is then mined on that pool.

### 7. Overclocking (RTX 3070)

The kernel works from the L2 cache and needs almost no VRAM bandwidth, so the memory clock can go down.

| Setting | Value |
|---|---|
| Core clock | locked, `1560` MHz |
| Memory clock | `810` MHz (or the lowest your card accepts) |
| Power limit | `120` W |

Result: **~70 TH/s at ~105–107 W**.

* HiveOS: Overclock tab → core `1560` absolute, memory `810` absolute, power limit `120`.
* Linux: `nvidia-smi -pl 120`, `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810` (root).
* Windows: MSI Afterburner, or an elevated `nvidia-smi -lgc 1560,1560` and `nvidia-smi -pl 120`.

Other 30-series cards: start from the same values and adjust. If shares get rejected or the log shows `bad` hits, lower the core clock.

### 8. Reading the log

| Line | Meaning |
|---|---|
| `FreeForGemPear 1.3.1 \| pool ... \| worker ... \| N GPU(s)` | start banner |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | the tuned kernel is loaded. `embedded search kernel could not be loaded (reason)` = the miner stops with an error (driver older than 525 or an unsupported GPU) |
| `new job ... prepared in ...` | a new pool job is ready |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | every 10 s: hashrate per GPU and in total; shares submitted / accepted / rejected for you and for the dev fee; `[DEVFEE]` while a fee slot runs |
| `[DISCONNECTED]` | the pool connection is down; the miner reconnects by itself |
| `devfee: ...` | the dev fee slots (section 10) |
| `!! ...` | a warning or error (a GPU worker stopped, a hit was not submitted) |

### 9. Verifying the download (SHA-256)

Compare the result with the hash on the release page.

* Linux: `sha256sum freeforgempear-1.3.1-linux-x86_64.tar.gz freeforgempear-1.3.1.tar.gz`
* Windows: `certutil -hashfile freeforgempear-1.3.1-windows-x64.zip SHA256` or
  `Get-FileHash freeforgempear-1.3.1-windows-x64.zip -Algorithm SHA256`

### 10. Dev fee — 1%

One 60-second slot in every 100 minutes, at a random moment of the cycle, on **the same pool you configured**, under a
separate login (worker `ffgp-devfee`). The log shows `devfee: next slot in N min`,
`devfee: mining 60s for FreeForGemPear dev (1%)` and `devfee: back to user`. The first slot comes at a random moment within
the first 100 minutes. If the fee login cannot be served, the slot is skipped; your mining is never blocked.
The fee is a condition of the license: removing, reducing or redirecting it is not permitted (see [LICENSE](LICENSE), clause 3).

### 11. Troubleshooting

| Symptom | Fix |
|---|---|
| Check that everything works | `freeforgempear --self-test` (Windows: `freeforgempear.exe --self-test`) must end with `== self-test PASSED ==` |
| `embedded search kernel could not be loaded` at start | update the NVIDIA driver (525+) and check that the GPU is Ampere (RTX 30xx) or newer |
| High CPU load | by default GPU waits sleep (well under 10% of a core per GPU). `PEARL_SPIN=1` makes them spin (a full core per GPU): make sure it is not set |
| `pg_init(...)` or out-of-memory errors | another program holds the GPU (games, local AI models). Close it or choose GPUs with `--devices` |
| `[DISCONNECTED]` for a long time | check the pool host/port and the firewall (outgoing TCP); try another region from section 6 |
| Many rejected shares | lower the core clock (section 7), check cooling |
| HiveOS says `Already installed` and nothing changes | the Installation URL must point to a new version |
| Windows: the window closes at once | run `start.bat` from `cmd` to see the message; the path must not contain spaces or non-English letters |
| Windows: `.exe` blocked by Defender / SmartScreen | *More info → Run anyway*, or add the folder to the exclusions; check the SHA-256 |

Environment variables (set before starting the miner): `PEARL_SPIN=1` (spin-wait instead of sleeping).
On Windows: `set PEARL_SPIN=1` in a `cmd` window, then run `start.bat` from that window.

---

## Русский

### 1. Что это

FreeForGemPear — GPU-майнер **Pearl (PRL)** (pearlhash, proof-of-useful-work) для видеокарт NVIDIA с **комиссией разработчика 1%**.
Собственное CUDA-ядро для Ampere: матрица A′ генерируется на кристалле, int8-GEMM на тензорных ядрах идёт в
персистентном ядре, а свёртка jackpot и хеш BLAKE3 слиты с ним. Главный цикл настроен вручную на уровне SASS.
Хост строит и проверяет доказательство каждой шары официальным верификатором `zk-pow` до отправки.

| Серия | Статус |
|---|---|
| RTX 30xx (Ampere, sm_86): 3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **основная цель, оптимизировано: ~70 TH/s на RTX 3070 при ~107 Вт** |
| RTX 40xx (Ada, sm_89) | запускает код sm_86, без оптимизации |
| Старые карты, A100 (sm_80), AMD | не поддерживаются |

### 2. Требования

* Видеокарта NVIDIA серии RTX 30 (RTX 40 — без оптимизации), драйвер NVIDIA **525 или новее**.
* HiveOS, Linux x86_64 (glibc 2.31+, Ubuntu 20.04+) или Windows 10/11 x64. CUDA toolkit, Python и WSL не нужны.
* Адрес кошелька PRL (`prl1...`) и интернет до stratum-пула Pearl.
* Видеопамять: ядро работает из L2-кэша и почти не использует VRAM.

### 3. Настройка в HiveOS (по шагам)

FreeForGemPear запускается в HiveOS как **Custom miner**: SSH и ручная установка не нужны.

**Ссылка для установки (1.3.1):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.1/freeforgempear-1.3.1.tar.gz
```

1. **Кошелёк.** В разделе *Wallets* добавьте кошелёк PRL: монета `PRL`, адрес `prl1...`.
2. **Новый полётный лист.**
   * **Coin:** `PRL`.
   * **Wallet:** кошелёк из шага 1.
   * **Pool:** *Configure in miner*.
   * **Miner:** *Custom*, затем *Setup Miner Config*.
3. **Настройки майнера:**

   | Поле | Значение |
   |---|---|
   | Miner name | `freeforgempear` (подставится сам после вставки ссылки) |
   | Installation URL | ссылка выше |
   | Hash algorithm | `pearlhash` (или оставить `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360` (или любой пул из раздела 6) |
   | Pass | пусто |
   | Extra config arguments | по желанию, например `--devices 0,1` |

4. **Применить.** *Apply changes*, *Create flight sheet*, затем применить лист к ригу.

Лог майнера: `/var/log/miner/freeforgempear/freeforgempear.log` и консоль майнера в HiveOS.
На панели видны хэшрейт по картам и общий, температура, вентиляторы, принятые и отклонённые шары.
Для обновления смените Installation URL на новую версию. То же подробнее: [HIVEOS.md](HIVEOS.md).

### 4. Linux без HiveOS

```
tar xzf freeforgempear-1.3.1-linux-x86_64.tar.gz
cd freeforgempear-1.3.1
./freeforgempear --self-test        # по желанию: сверка GPU-ядра с CPU-эталоном
./freeforgempear --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1
```

| Опция | Значение |
|---|---|
| `--pool HOST:PORT` | stratum-пул (по умолчанию `pearl-eu2.luckypool.io:3360`) |
| `--wallet ADDRESS` | ваш адрес PRL (обязательно) |
| `--worker NAME` | имя воркера (по умолчанию `worker1`) |
| `--devices 0,1,...` | какие GPU использовать (по умолчанию все видимые; учитывает `CUDA_VISIBLE_DEVICES`) |
| `--minutes N` | остановиться через N минут (по умолчанию работает до Ctrl-C / SIGTERM) |
| `--self-test` | проверить ядро и выйти |

Один процесс ведёт все GPU и сам переподключается к пулу. Для постоянной работы запускайте под `systemd`, `screen` или `tmux`.

### 5. Windows 10/11

Нативная сборка: WSL, установка CUDA и Python не нужны. Нужен только драйвер NVIDIA.

1. Скачайте **`freeforgempear-1.3.1-windows-x64.zip`** со страницы релиза:
   `https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.1/freeforgempear-1.3.1-windows-x64.zip`
   и сверьте SHA-256 (раздел 9).
2. Распакуйте в папку **без пробелов и русских букв**, например `C:\FreeForGemPear`.
3. Правой кнопкой по **`start.bat`** → *Изменить* и задайте:

| Параметр | Значение |
|---|---|
| `WALLET` | ваш адрес PRL (`prl1...`) |
| `POOL` | `HOST:PORT`, по умолчанию `pearl-eu2.luckypool.io:3360` (раздел 6) |
| `WORKER` | имя рига, по умолчанию имя компьютера |
| `DEVICES` | пусто = все GPU NVIDIA, или список вида `0,1` (индексы из `nvidia-smi`) |

4. Запустите **`start.bat`**. Один процесс майнера ведёт все GPU; вывод идёт в окно и в файл
   `logs\miner-<дата>.log` (хранятся последние 10 логов). Если майнер завершился или упал, он перезапускается через 15 с.
   `Ctrl+C` — остановка.

* **Автозапуск:** положите ярлык `start.bat` в `shell:startup` (Win+R, `shell:startup`) или создайте задачу в Планировщике
  «При входе в систему», запускающую `start.bat`.
* **Разгон:** MSI Afterburner (фиксация ядра в редакторе кривой, память вниз) со значениями из раздела 7.
  Ноутбуки: подключите зарядку и выберите режим максимальной производительности.
* Defender / SmartScreen может ругаться на новый неподписанный `.exe`: *Подробнее → Выполнить в любом случае* или добавьте папку в исключения.
* Без `start.bat`: `freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet ВАШ_АДРЕС_PRL --worker rig1`
* Собрать самому: `.\build-windows.ps1` (см. README).

### 6. Пулы

По умолчанию **LuckyPool**, `pearl-eu2.luckypool.io:3360` (stratum по TCP). Хосты и порты ниже взяты из конфигурации самого пула
(`https://pearl.luckypool.io/api/stats`, `config.stratum` и `config.ports`); выбирайте ближайший регион:

| Регион | Хост |
|---|---|
| Европа | `pearl-eu2.luckypool.io` (по умолчанию), `pearl-eu1.luckypool.io`, `pearl-pl.luckypool.io` (Польша), `pearl-tr.luckypool.io` (Турция) |
| Россия | `pearl-ru.luckypool.io` |
| США | `pearl-us-east.luckypool.io`, `pearl-us-west.luckypool.io`, `pearl-us-central.luckypool.io`, `pearl-us-ord.luckypool.io` |
| Южная Америка | `pearl-br.luckypool.io` (Бразилия) |
| Канада | `pearl-ca1.luckypool.io`, `pearl-ca2.luckypool.io` |
| Азия | `pearl-sg1.luckypool.io`, `pearl-sg2.luckypool.io` (Сингапур), `pearl-id.luckypool.io` (Индонезия), `pearl-hk.luckypool.io` (Гонконг), `pearl-in.luckypool.io` (Индия), `pearl-jp.luckypool.io` (Япония), `pearl-au.luckypool.io` (Австралия) |

| Порт | Сложность шары (vardiff) |
|---|---|
| `3360` | 2 000 000 (по умолчанию) |
| `3361` | 4 000 000 |
| `3362` | 8 000 000 |

Пример: `--pool pearl-pl.luckypool.io:3360`. В качестве кошелька указывайте обычный адрес `prl1...`, имя воркера передаётся отдельно.
Подойдёт любой другой stratum-пул Pearl с тем же JSON-RPC; комиссия (раздел 10) тогда майнится на этом пуле.

### 7. Разгон (RTX 3070)

Ядро работает из L2-кэша и почти не использует пропускную способность видеопамяти, поэтому частоту памяти можно снизить.

| Параметр | Значение |
|---|---|
| Частота ядра | зафиксирована, `1560` МГц |
| Частота памяти | `810` МГц (или минимальная, которую принимает карта) |
| Лимит мощности | `120` Вт |

Результат: **~70 TH/s при ~105–107 Вт**.

* HiveOS: вкладка Overclock → ядро `1560` абсолютное, память `810` абсолютная, лимит мощности `120`.
* Linux: `nvidia-smi -pl 120`, `nvidia-smi -lgc 1560,1560`, `nvidia-smi -lmc 810,810` (root).
* Windows: MSI Afterburner или `nvidia-smi -lgc 1560,1560` и `nvidia-smi -pl 120` от администратора.

Другие карты 30-й серии: начните с тех же значений и подберите под свою карту. Если шары отклоняются или в логе есть `bad`, снизьте частоту ядра.

### 8. Как читать лог

| Строка | Значение |
|---|---|
| `FreeForGemPear 1.3.1 \| pool ... \| worker ... \| N GPU(s)` | стартовая строка |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | загружено настроенное ядро. `embedded search kernel could not be loaded (причина)` = майнер останавливается с ошибкой (драйвер старше 525 или неподдерживаемая карта) |
| `new job ... prepared in ...` | готово новое задание пула |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | каждые 10 с: хэшрейт по картам и общий; отправленные / принятые / отклонённые шары ваши и комиссии; `[DEVFEE]` пока идёт слот комиссии |
| `[DISCONNECTED]` | соединение с пулом потеряно; майнер переподключится сам |
| `devfee: ...` | слоты комиссии (раздел 10) |
| `!! ...` | предупреждение или ошибка (остановился GPU-воркер, hit не отправлен) |

### 9. Проверка загрузки (SHA-256)

Сравните результат с хешем на странице релиза.

* Linux: `sha256sum freeforgempear-1.3.1-linux-x86_64.tar.gz freeforgempear-1.3.1.tar.gz`
* Windows: `certutil -hashfile freeforgempear-1.3.1-windows-x64.zip SHA256` или
  `Get-FileHash freeforgempear-1.3.1-windows-x64.zip -Algorithm SHA256`

### 10. Комиссия — 1%

Один 60-секундный слот в каждом 100-минутном цикле, в случайный момент цикла, на **том же пуле, который вы указали**, под отдельным
логином (воркер `ffgp-devfee`). В логе: `devfee: next slot in N min`, `devfee: mining 60s for FreeForGemPear dev (1%)`,
`devfee: back to user`. Первый слот приходится на случайный момент первых 100 минут. Если логин комиссии не обслуживается, слот
пропускается; ваш майнинг не блокируется. Комиссия — условие лицензии: убирать, уменьшать или перенаправлять её нельзя
(см. [LICENSE](LICENSE), пункт 3).

### 11. Проблемы и решения

| Симптом | Решение |
|---|---|
| Проверить, что всё работает | `freeforgempear --self-test` (Windows: `freeforgempear.exe --self-test`) должен закончиться строкой `== self-test PASSED ==` |
| При старте `embedded search kernel could not be loaded` | обновите драйвер NVIDIA (525+) и проверьте, что карта Ampere (RTX 30xx) или новее |
| Высокая загрузка CPU | по умолчанию ожидание GPU спит (заметно меньше 10% ядра на карту). `PEARL_SPIN=1` включает активное ожидание (ядро CPU на карту): убедитесь, что она не задана |
| Ошибки `pg_init(...)` или нехватка памяти | видеокарту занимает другая программа (игры, локальные нейросети). Закройте её или выберите карты через `--devices` |
| Долго `[DISCONNECTED]` | проверьте хост/порт пула и брандмауэр (исходящий TCP); попробуйте другой регион из раздела 6 |
| Много отклонённых шаров | снизьте частоту ядра (раздел 7), проверьте охлаждение |
| HiveOS пишет `Already installed` и ничего не меняется | Installation URL должен указывать на новую версию |
| Windows: окно сразу закрывается | запустите `start.bat` из `cmd`, чтобы увидеть сообщение; в пути не должно быть пробелов и русских букв |
| Windows: `.exe` блокирует Defender / SmartScreen | *Подробнее → Выполнить в любом случае* или добавьте папку в исключения; сверьте SHA-256 |

Переменные окружения (задаются до запуска майнера): `PEARL_SPIN=1` (активное ожидание вместо сна).
В Windows: `set PEARL_SPIN=1` в окне `cmd`, затем запустите `start.bat` из этого же окна.

---

## 中文

### 1. 这是什么

FreeForGemPear 是面向 NVIDIA 显卡的 **Pearl (PRL)**（pearlhash，有用工作量证明）GPU 矿工，开发者抽水 **1%**。
它使用针对 Ampere 的自研 CUDA 内核：A′ 矩阵在芯片上生成，int8 张量核心 GEMM 在常驻内核中运行，jackpot 折叠和 BLAKE3
哈希都融合在其中。主循环在 SASS 级别手工调优。提交之前，主机会用官方 `zk-pow` 验证器构建并检查每个份额的证明。

| 显卡系列 | 状态 |
|---|---|
| RTX 30xx（Ampere，sm_86）：3060 / 3060 Ti / 3070 / 3070 Ti / 3080 / 3090 | **主要目标，已调优：RTX 3070 约 70 TH/s，约 107 W** |
| RTX 40xx（Ada，sm_89） | 运行 sm_86 代码，未调优 |
| 更老的显卡、A100（sm_80）、AMD | 不支持 |

### 2. 系统要求

* RTX 30 系列 NVIDIA 显卡（RTX 40 未调优可运行），NVIDIA 驱动 **525 或更新**。
* HiveOS、Linux x86_64（glibc 2.31+，Ubuntu 20.04+）或 Windows 10/11 x64。不需要 CUDA toolkit、Python 或 WSL。
* PRL 钱包地址（`prl1...`）以及到 Pearl stratum 矿池的网络连接。
* 显存：内核从 L2 缓存工作，几乎不占用显存。

### 3. HiveOS 设置（分步说明）

FreeForGemPear 在 HiveOS 中作为 **Custom miner** 运行：无需 SSH，无需手动安装。

**安装链接 (1.3.1)：**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.1/freeforgempear-1.3.1.tar.gz
```

1. **钱包。** 在 *Wallets* 中添加 PRL 钱包：币种 `PRL`，你的 `prl1...` 地址。
2. **新建 Flight sheet。**
   * **Coin：** `PRL`。
   * **Wallet：** 第 1 步的钱包。
   * **Pool：** *Configure in miner*。
   * **Miner：** *Custom*，然后点击 *Setup Miner Config*。
3. **矿工配置：**

   | 字段 | 值 |
   |---|---|
   | Miner name | `freeforgempear`（粘贴链接后自动填入） |
   | Installation URL | 上面的链接 |
   | Hash algorithm | `pearlhash`（或保持 `----`） |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`（或第 6 节中的任一矿池） |
   | Pass | 留空 |
   | Extra config arguments | 可选，例如 `--devices 0,1` |

4. **应用。** *Apply changes*，*Create flight sheet*，然后应用到矿机。

矿工日志位于 `/var/log/miner/freeforgempear/freeforgempear.log`，也可在 HiveOS 矿工控制台查看。
仪表盘显示每块显卡和总算力、温度、风扇，以及已接受和被拒绝的份额。更新时把 Installation URL 改成新版本即可。
更详细的同一指南：[HIVEOS.md](HIVEOS.md)。

### 4. 不使用 HiveOS 的 Linux

```
tar xzf freeforgempear-1.3.1-linux-x86_64.tar.gz
cd freeforgempear-1.3.1
./freeforgempear --self-test        # 可选：将 GPU 内核与 CPU 参考实现比对
./freeforgempear --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1
```

| 选项 | 含义 |
|---|---|
| `--pool HOST:PORT` | stratum 矿池（默认 `pearl-eu2.luckypool.io:3360`） |
| `--wallet ADDRESS` | 你的 PRL 地址（必填） |
| `--worker NAME` | 矿工名（默认 `worker1`） |
| `--devices 0,1,...` | 使用的显卡（默认所有可见显卡；遵循 `CUDA_VISIBLE_DEVICES`） |
| `--minutes N` | N 分钟后停止（默认一直运行到 Ctrl-C / SIGTERM） |
| `--self-test` | 验证内核后退出 |

一个进程驱动所有显卡，并自动重连矿池。长期运行请放在 `systemd`、`screen` 或 `tmux` 下。

### 5. Windows 10/11

原生 Windows 版本：无需 WSL、无需安装 CUDA、无需 Python。只需要 NVIDIA 驱动。

1. 从发布页面下载 **`freeforgempear-1.3.1-windows-x64.zip`**：
   `https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.1/freeforgempear-1.3.1-windows-x64.zip`
   并核对 SHA-256（第 9 节）。
2. 解压到**不含空格和非英文字符**的文件夹，例如 `C:\FreeForGemPear`。
3. 右键点击 **`start.bat`** → *编辑*，并设置：

| 设置项 | 值 |
|---|---|
| `WALLET` | 你的 PRL 地址（`prl1...`） |
| `POOL` | `HOST:PORT`，默认 `pearl-eu2.luckypool.io:3360`（第 6 节） |
| `WORKER` | 矿机名称，默认 = 计算机名 |
| `DEVICES` | 留空 = 所有 NVIDIA 显卡，或类似 `0,1` 的列表（编号来自 `nvidia-smi`） |

4. 双击 **`start.bat`**。一个矿工进程驱动所有显卡；输出显示在窗口中并保存到
   `logs\miner-<日期>.log`（保留最近 10 个日志）。矿工退出或崩溃后会在 15 秒后自动重启。`Ctrl+C` 停止。

* **开机自启：** 把 `start.bat` 的快捷方式放进 `shell:startup`（Win+R，输入 `shell:startup`），或在任务计划程序中创建"登录时"运行 `start.bat` 的任务。
* **超频：** 使用 MSI Afterburner（在曲线编辑器中锁定核心频率，降低显存频率），数值见第 7 节。笔记本：接上充电器并选择最高性能模式。
* Defender / SmartScreen 可能拦截新的未签名 `.exe`：点击"更多信息 → 仍要运行"，或将该文件夹加入排除项。
* 不使用 `start.bat`：`freeforgempear.exe --pool pearl-eu2.luckypool.io:3360 --wallet 你的PRL地址 --worker rig1`
* 自行编译：`.\build-windows.ps1`（见 README）。

### 6. 矿池

默认 **LuckyPool**，`pearl-eu2.luckypool.io:3360`（TCP stratum）。下面的主机和端口来自矿池自身配置
（`https://pearl.luckypool.io/api/stats` 中的 `config.stratum` 和 `config.ports`）；请选择离你最近的区域：

| 区域 | 主机 |
|---|---|
| 欧洲 | `pearl-eu2.luckypool.io`（默认）、`pearl-eu1.luckypool.io`、`pearl-pl.luckypool.io`（波兰）、`pearl-tr.luckypool.io`（土耳其） |
| 俄罗斯 | `pearl-ru.luckypool.io` |
| 美国 | `pearl-us-east.luckypool.io`、`pearl-us-west.luckypool.io`、`pearl-us-central.luckypool.io`、`pearl-us-ord.luckypool.io` |
| 南美 | `pearl-br.luckypool.io`（巴西） |
| 加拿大 | `pearl-ca1.luckypool.io`、`pearl-ca2.luckypool.io` |
| 亚洲 | `pearl-sg1.luckypool.io`、`pearl-sg2.luckypool.io`（新加坡）、`pearl-id.luckypool.io`（印尼）、`pearl-hk.luckypool.io`（香港）、`pearl-in.luckypool.io`（印度）、`pearl-jp.luckypool.io`（日本）、`pearl-au.luckypool.io`（澳大利亚） |

| 端口 | 份额难度（vardiff） |
|---|---|
| `3360` | 2 000 000（默认） |
| `3361` | 4 000 000 |
| `3362` | 8 000 000 |

示例：`--pool pearl-pl.luckypool.io:3360`。钱包填写普通的 `prl1...` 地址，矿工名单独发送。
也可使用其他使用相同 JSON-RPC stratum 的 Pearl 矿池；此时开发者抽水（第 10 节）在该矿池上进行。

### 7. 超频（RTX 3070）

内核从 L2 缓存工作，几乎不需要显存带宽，因此可以降低显存频率。

| 设置 | 值 |
|---|---|
| 核心频率 | 锁定 `1560` MHz |
| 显存频率 | `810` MHz（或显卡能接受的最低值） |
| 功耗上限 | `120` W |

结果：**约 70 TH/s，约 105–107 W**。

* HiveOS：Overclock 标签 → 核心 `1560` 绝对值，显存 `810` 绝对值，功耗上限 `120`。
* Linux：`nvidia-smi -pl 120`、`nvidia-smi -lgc 1560,1560`、`nvidia-smi -lmc 810,810`（root）。
* Windows：MSI Afterburner，或以管理员身份运行 `nvidia-smi -lgc 1560,1560` 和 `nvidia-smi -pl 120`。

其他 30 系列显卡：从相同数值开始再按显卡调整。如果份额被拒绝或日志中出现 `bad`，请降低核心频率。

### 8. 如何阅读日志

| 行 | 含义 |
|---|---|
| `FreeForGemPear 1.3.1 \| pool ... \| worker ... \| N GPU(s)` | 启动横幅 |
| `[pearlgpu] search kernel: SASS-patched cubin (...)` | 已加载调优内核。`embedded search kernel could not be loaded (原因)` = 矿工报错退出（驱动低于 525 或显卡不受支持） |
| `new job ... prepared in ...` | 新的矿池任务已就绪 |
| `[ 120s] GPU0 ... \| total X TH/s \| hits ... \| user sub S acc A rej R \| fee sub ... acc ... rej ...` | 每 10 秒：每块显卡和总算力；你和抽水的已提交 / 已接受 / 已拒绝份额；抽水时段显示 `[DEVFEE]` |
| `[DISCONNECTED]` | 与矿池断开；矿工会自动重连 |
| `devfee: ...` | 抽水时段（第 10 节） |
| `!! ...` | 警告或错误（GPU 工作线程停止、某个 hit 未提交） |

### 9. 校验下载（SHA-256）

将结果与发布页面上的哈希比对。

* Linux：`sha256sum freeforgempear-1.3.1-linux-x86_64.tar.gz freeforgempear-1.3.1.tar.gz`
* Windows：`certutil -hashfile freeforgempear-1.3.1-windows-x64.zip SHA256` 或
  `Get-FileHash freeforgempear-1.3.1-windows-x64.zip -Algorithm SHA256`

### 10. 开发者抽水 — 1%

每 100 分钟一个 60 秒时段，位于周期内的随机时刻，在**你配置的同一个矿池**上，使用单独的登录（矿工名 `ffgp-devfee`）。
日志显示 `devfee: next slot in N min`、`devfee: mining 60s for FreeForGemPear dev (1%)` 和 `devfee: back to user`。
第一个时段出现在最初 100 分钟内的随机时刻。如果抽水登录无法服务，该时段会被跳过；你的挖矿不会被阻塞。
抽水是许可证的条件：不得移除、降低或重定向（见 [LICENSE](LICENSE) 第 3 条）。

### 11. 故障排除

| 症状 | 解决办法 |
|---|---|
| 检查一切是否正常 | `freeforgempear --self-test`（Windows：`freeforgempear.exe --self-test`）应以 `== self-test PASSED ==` 结束 |
| 启动时出现 `embedded search kernel could not be loaded` | 更新 NVIDIA 驱动（525+），并确认显卡为 Ampere（RTX 30xx）或更新 |
| CPU 占用高 | 默认 GPU 等待为休眠（每块显卡远低于一个核心的 10%）。`PEARL_SPIN=1` 会改为忙等（每块显卡占满一个核心）：确认没有设置 |
| `pg_init(...)` 或显存不足错误 | 其他程序占用了显卡（游戏、本地 AI 模型）。关闭它，或用 `--devices` 选择显卡 |
| 长时间 `[DISCONNECTED]` | 检查矿池主机/端口和防火墙（出站 TCP）；试试第 6 节的其他区域 |
| 大量被拒绝的份额 | 降低核心频率（第 7 节），检查散热 |
| HiveOS 提示 `Already installed` 且没有变化 | Installation URL 必须指向新版本 |
| Windows：窗口一闪而过 | 在 `cmd` 中运行 `start.bat` 查看信息；路径不得含空格和非英文字符 |
| Windows：`.exe` 被 Defender / SmartScreen 拦截 | 点击"更多信息 → 仍要运行"，或将文件夹加入排除项；核对 SHA-256 |

环境变量（启动矿工前设置）：`PEARL_SPIN=1`（忙等代替休眠）。
Windows：在 `cmd` 窗口中 `set PEARL_SPIN=1`，然后在同一窗口中运行 `start.bat`。
