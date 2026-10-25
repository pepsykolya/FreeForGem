# FreeForGem 1.7.0-beta — release notes

[English](#english) · [Русский](#русский)

## English

**FreeForGem is the new name of the multi-coin FreeForGemPear** (Pearl + Common Foundry + NOID). FreeForGemPear continues as
the Pearl miner under its own name and repository; the multi-coin line lives in `pepsykolya/FreeForGem`.

### What is new

* **NOID (Parano1d) mining** on NVIDIA RTX 30/40/50 and A100/H100 (sm_80+), **driver 580+** — `--algo noid` (or a NOID pool
  URL). Own closed-source kernels in a separate library (`libnoidgpu.so` / `noidgpu.dll`; the Pearl library is unchanged),
  two modes:
  * `--noid-mode eff` (default, best MH/W): RTX 3070 @1400 MHz + undervolt **34.7 MH/s @ 73 W = 0.47–0.48 MH/W (offset +195 found by `--auto-uv`; 0.393 at offset 0)**;
  * `--noid-mode speed` (best MH/s): RTX 3070 **43.65 MH/s @1560 MHz, 46.16 MH/s @1650 MHz**; RTX 4070 Ti 82.6 MH/s at stock clocks (power limit 190 W).
* Pool protocol `parano1d-stratum-v1` over **TLS** (`stratum+ssl://`), tested on InnovLab EU
  (`stratum+ssl://eu2.innovlab.cc:19601`): all shares accepted in our runs (user 63/63, 41/41, 31/31, 24/24; dev fee 29/29), RTX 4070 Ti 32/32 and 51/51.
* Every GPU candidate is **re-computed on the CPU with the official `noid_poseidon2b` crate** before it is submitted;
  `--algo noid --self-test` checks the 16 official test vectors and 32768 GPU digests per kernel against the crate, and a full
  search launch's candidate set.
* **`--auto-uv` for NOID**: its own per-card and per-mode undervolt, every step checked by a 40 s bit-exact stress check
  (thousands of launches compared with the CPU), one step below the first failure, 2-minute confirmation, saved in `uv.json`
  under `noid`; while mining, a CPU check of every candidate plus a watchdog check every 120 s step the offset back on a
  failure. Without `--auto-uv` a positive (Pearl) offset is set to 0 for NOID and put back on exit. RTX 3070 result: eff @1400 MHz 87.5 → 72.8 W (+20 % MH/W, voltage floor at +195), speed @1560 MHz 127.8 → 106.2 W (+20 % MH/W, silent edge at +225: 5 of 4597 launches wrong, no Xid).
* Dev fee 1% for NOID, the same schedule as Pearl (60 s every 100 min, same pool).
* HiveOS: custom miner **`freeforgem`** — **create a new flight sheet** (Miner name `freeforgem`, the new Installation URL);
  NOID: hash algorithm `parano1d`, hashrate in kH/s. Windows: `start-noid.bat`.
* Saved undervolt results of FreeForGemPear (`uv.json`) are imported automatically on the first start.
* New manual sections: **15 NOID** and **16 The silent edge of undervolt** (EN/RU/中文).

### The silent edge of undervolt

Near the stability limit a GPU does not crash, reports no Xid and passes a quick self-test, yet some of its results are wrong
(NOID, RTX 3070 @1560 MHz, offset +250: 168 of 5984 launches wrong, the first after ~12 s, no driver error). That costs
invalid or never-found shares that the miner log does not show. FreeForGem tunes with bit-exact checks over thousands of
launches under full load (Pearl: tile digests + CPU verification; NOID: the long stress check; CMFD: digests on the real model),
takes the result one step below the first failure, confirms it, and keeps checking while mining — a failure steps the offset
back automatically. MANUAL.md, section 16.

### Requirements and limits

* NOID: NVIDIA sm_80+ and driver **580+** (older drivers are refused with a clear message: loading the CUDA 13 kernels on a
  575 driver crashes the process). No Turing, no AMD for NOID.
* Pearl and CMFD are unchanged from FreeForGemPear 1.6.2 (same Pearl library, same CMFD engine).
* Not tested in this beta: NOID on Windows hardware (the build machine has driver 576), NOID on RTX 50.
* NOID dev-fee address: the developer's NOID wallet (`o1qxh67…gmhdc`).

### Files

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.0beta.tar.gz` — HiveOS custom miner (Installation URL) | `7e0773fb2e7fb92fe87ed331d72155a82454b2dcb15f6fd2d0e965d171d5f7f3` |
| `freeforgem-1.7.0-beta-linux-x86_64.tar.gz` — Linux x86_64 (glibc 2.31+) | `57fd46ab5a1057befef4d79dfdb89120be6a2dcc6188caa351a41720691a02f5` |
| `freeforgem-1.7.0-beta-windows-x64.zip` — Windows 10/11 x64 | `c3d7ba12af3966f8afd37527a7d85eb313ef1e92e125409154c0b2b07f529811` |

Inside: `libnoidgpu.so` sha256 `6da4416214fa00f8695501634dc99fa4af5db35019060abbf7e3170adea9e37c`, `noidgpu.dll` `4d90c1c1d3b70fb1b7ae7118fd1a03aec9be5f9de5dabf18811a8b6714ab2b2f`; the Pearl libraries are byte-identical to 1.6.2.

---

## Русский

**FreeForGem — новое имя мульти-монетного FreeForGemPear** (Pearl + Common Foundry + NOID). FreeForGemPear остаётся майнером
Pearl под своим именем и в своём репозитории; мульти-монетная линейка — в `pepsykolya/FreeForGem`.

### Что нового

* **Майнинг NOID (Parano1d)** на NVIDIA RTX 30/40/50 и A100/H100 (sm_80+), **драйвер 580+** — `--algo noid` (или URL
  NOID-пула). Свои закрытые ядра в отдельной библиотеке (`libnoidgpu.so` / `noidgpu.dll`; библиотека Pearl не менялась),
  два режима:
  * `--noid-mode eff` (по умолчанию, лучшие MH/Вт): RTX 3070 @1400 МГц + андервольт **34.7 MH/s @ 73 Вт = 0.47–0.48 MH/Вт (offset +195, найден `--auto-uv`; 0.393 на offset 0)**;
  * `--noid-mode speed` (максимум MH/s): RTX 3070 **43.65 MH/s @1560 МГц, 46.16 MH/s @1650 МГц**; RTX 4070 Ti 82.6 MH/s на стоковых частотах (лимит 190 Вт).
* Протокол пула `parano1d-stratum-v1` через **TLS** (`stratum+ssl://`), проверен на InnovLab EU
  (`stratum+ssl://eu2.innovlab.cc:19601`): в наших прогонах все шары приняты (user 63/63, 41/41, 31/31, 24/24; комиссия 29/29), RTX 4070 Ti 32/32 и 51/51.
* Каждый кандидат GPU **пересчитывается на CPU официальным крейтом `noid_poseidon2b`** перед отправкой;
  `--algo noid --self-test` проверяет 16 официальных тест-векторов и 32768 дайджестов GPU на каждое ядро против крейта, а также
  набор кандидатов полного запуска поиска.
* **`--auto-uv` для NOID**: свой андервольт на каждую карту и режим, каждый шаг — 40-секундная побайтная stress-проверка (тысячи
  запусков против CPU), результат на шаг ниже первой ошибки, подтверждение 2 минуты, сохраняется в `uv.json` в разделе `noid`;
  в майнинге — CPU-проверка каждого кандидата и сторожевая проверка каждые 120 с, при ошибке offset откатывается. Без
  `--auto-uv` положительный (Pearl-овский) offset на время NOID ставится в 0 и возвращается при выходе. RTX 3070: eff @1400 МГц 87.5 → 72.8 Вт (+20 % MH/Вт, пол напряжения на +195), speed @1560 МГц 127.8 → 106.2 Вт (+20 % MH/Вт, тихий край на +225: 5 из 4597 запусков с ошибкой, Xid нет).
* Комиссия 1 % для NOID — по тому же расписанию, что у Pearl (60 с каждые 100 мин, тот же пул).
* HiveOS: кастомный майнер **`freeforgem`** — **создайте новый полётный лист** (Miner name `freeforgem`, новая ссылка
  установки); NOID: hash algorithm `parano1d`, хэшрейт в kH/s. Windows: `start-noid.bat`.
* Сохранённые результаты андервольта FreeForGemPear (`uv.json`) подхватываются автоматически при первом запуске.
* Новые разделы руководства: **15 NOID** и **16 «Тихий край» андервольта** (EN/RU/中文).

### «Тихий край» андервольта

Вблизи предела устойчивости GPU не падает, не сообщает Xid и проходит быстрый self-test, но часть результатов уже неверна
(NOID, RTX 3070 @1560 МГц, offset +250: 168 из 5984 запусков с ошибкой, первая через ~12 с, без ошибок драйвера). Это
невалидные или так и не найденные шары, которых не видно в логе майнера. FreeForGem подбирает offset с побайтными проверками
тысяч запусков под полной нагрузкой (Pearl: дайджесты тайлов + CPU-проверка; NOID: длинная stress-проверка; CMFD: дайджесты на
настоящей модели), берёт результат на шаг ниже первой ошибки, подтверждает его и продолжает проверять в майнинге — ошибка
автоматически откатывает offset. MANUAL.md, раздел 16.

### Требования и ограничения

* NOID: NVIDIA sm_80+ и драйвер **580+** (со старым драйвером — понятный отказ: загрузка ядер CUDA 13 на драйвере 575 роняет
  процесс). Turing и AMD для NOID не поддерживаются.
* Pearl и CMFD не изменились относительно FreeForGemPear 1.6.2 (та же библиотека Pearl, тот же движок CMFD).
* Не проверено в этой бете: NOID на Windows-железе (на сборочной машине драйвер 576), NOID на RTX 50.
* Адрес комиссии NOID: NOID-кошелёк разработчика (`o1qxh67…gmhdc`).
