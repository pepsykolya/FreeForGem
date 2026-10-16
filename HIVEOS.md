# FreeForGemPear on HiveOS

[English](#english) · [Русский](#русский)

## English

FreeForGemPear runs on HiveOS as a **Custom miner**: no SSH and no manual installation.

**Installation URL (1.5.0):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.0/freeforgempear-1.5.0.tar.gz
```

### Flight sheet

1. **Wallet.** In *Wallets*, add your PRL wallet: coin `PRL`, your `prl1...` address. On LuckyPool you can merge-mine NOCK as well:
   use `prl1...+YOUR_NOCK_ADDRESS` as the address (LuckyPool only; see MANUAL.md, section 6).
2. **New flight sheet.**
   - **Coin:** `PRL`.
   - **Wallet:** the wallet from step 1.
   - **Pool:** *Configure in miner*.
   - **Miner:** *Custom*, then press *Setup Miner Config*.
3. **Miner config:**

   | Field | Value |
   |---|---|
   | Miner name | `freeforgempear` (filled in automatically after you paste the URL) |
   | Installation URL | the link above |
   | Hash algorithm | `pearlhash` (or leave `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360`, or Kryptex `prl-eu.kryptex.network:7048` (or any other Pearl stratum pool) |
   | Pass | leave empty |
   | Extra config arguments | optional, for example `--devices 0,1`; `--auto-uv` enables the undervolt tuner (see below) |

4. **Apply.** *Apply changes*, *Create flight sheet*, then apply it to the rig.

The miner log is at `/var/log/miner/freeforgempear/freeforgempear.log` and in the HiveOS miner console.
The dashboard shows the hashrate per GPU and in total, temperatures, fans, and accepted and rejected shares.

### Overclocking (RTX 3070)

The kernel works from L2 cache and needs almost no VRAM bandwidth:

| Setting | Value |
|---|---|
| Core clock | absolute, `1560` MHz |
| Memory clock | absolute, `810` MHz (in HiveOS: Memory clock `810`, or the lowest your card accepts) |
| Power limit | `120` W |

- **1560 MHz:** ~70 TH/s at ~107 W.

### Undervolt (optional, `--auto-uv`, new in 1.4.0)

Off by default. Add `--auto-uv` to *Extra config arguments* (for example `--devices 0,1 --auto-uv`) and the miner finds, once per
GPU, the lowest core voltage (V/F offset) at which the card still computes correctly, applies it while mining and puts your
own settings back when it stops. Same hashrate, fewer watts (RTX 3070 at 1560 MHz: roughly 5-10 W less per card).

- It needs root, which HiveOS miners have. Without NVML or permission it logs the reason and mines normally.
- **The first tuning takes about 15 minutes per GPU** (10-25 depending on the number of steps) and the rig does not mine until it is done (GPUs are tuned in
  parallel). Later starts take about 30 s extra. The result is kept in `/hive-config/freeforgempear/uv.json` (survives miner
  updates and reboots); it is tuned again when the driver, the kernel build or the starting offset changes.
- Set your Overclock tab (core lock `1560`, memory, power limit) first. The tuner starts from the offset the card has when the
  miner starts and only goes up; it never touches locked clocks, power limit or memory. If HiveOS re-applies its OC and changes the offset
  while mining, auto-uv notices, stands down on that GPU and does not fight.
- It touches only the GPUs the miner mines on (matched by PCI bus id, shown in the log). Every change is logged as
  `auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved`; `grep auto-uv` in the miner log shows all of it.
- Stopping the miner restores your offset. If the miner was killed, the next start restores it first.
- **Reset:** run `freeforgempear --uv-reset` once in the miner folder (or delete `/hive-config/freeforgempear/uv.json`).
  Other options (`--uv-offset N`, `--uv-max N`, `--uv-reval-hours H`, `--auto-uv-dry-run`) and a manual-tuning
  procedure are in [MANUAL.md](MANUAL.md#12-undervolt---auto-uv-optional-new-in-140). Undervolting is at your own risk, like any overclocking.

### Requirements

- HiveOS with NVIDIA driver 525 or newer.
- Tuned for RTX 30xx. RTX 40xx should work but is untested.
- Older and AMD GPUs are not supported.

### Developer fee

1%: 60 s in every 100 minutes, on the same pool you set. See [LICENSE](LICENSE).

## Русский

FreeForGemPear запускается в HiveOS как **Custom miner**: SSH и ручная установка не нужны.

**Ссылка для установки (1.5.0):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.5.0/freeforgempear-1.5.0.tar.gz
```

### Полётный лист

1. **Кошелёк.** В разделе *Wallets* добавьте кошелёк PRL: монета `PRL`, адрес `prl1...`. На LuckyPool можно заодно майнить NOCK:
   укажите адрес в виде `prl1...+ВАШ_АДРЕС_NOCK` (только LuckyPool; см. MANUAL.md, раздел 6).
2. **Новый полётный лист.**
   - **Coin:** `PRL`.
   - **Wallet:** кошелёк из шага 1.
   - **Pool:** *Configure in miner*.
   - **Miner:** *Custom*, затем *Setup Miner Config*.
3. **Настройки майнера:**

   | Поле | Значение |
   |---|---|
   | Miner name | `freeforgempear` (подставится сам после вставки ссылки) |
   | Installation URL | ссылка выше |
   | Hash algorithm | `pearlhash` (или оставить `----`) |
   | Wallet and worker template | `%WAL%.%WORKER_NAME%` |
   | Pool URL | `pearl-eu2.luckypool.io:3360` или Kryptex `prl-eu.kryptex.network:7048` (или любой другой stratum-пул Pearl) |
   | Pass | пусто |
   | Extra config arguments | по желанию, например `--devices 0,1`; `--auto-uv` включает подбор андервольта (см. ниже) |

4. **Применить.** *Apply changes*, *Create flight sheet*, затем применить лист к ригу.

Лог майнера лежит в `/var/log/miner/freeforgempear/freeforgempear.log`, его видно и в консоли майнера в HiveOS.
На панели видны хэшрейт по картам и общий, температура, вентиляторы, принятые и отклонённые шары.

### Разгон (RTX 3070)

Ядро работает из L2-кэша, видеопамять ему почти не нужна:

| Параметр | Значение |
|---|---|
| Частота ядра | абсолютная, `1560` МГц |
| Частота памяти | абсолютная, `810` МГц (или минимальная, которую принимает карта) |
| Лимит мощности | `120` Вт |

- **1560 МГц:** ~70 TH/s при ~107 Вт.

### Андервольт (необязательно, `--auto-uv`, новое в 1.4.0)

По умолчанию выключено. Добавьте `--auto-uv` в *Extra config arguments* (например `--devices 0,1 --auto-uv`), и майнер один раз
для каждой карты найдёт минимальное напряжение ядра (V/F offset), при котором карта ещё считает правильно, применит его при
майнинге и вернёт ваши настройки при остановке. Тот же хешрейт, меньше ватт (RTX 3070 на 1560 МГц: примерно на 5-10 Вт меньше на карту).

- Нужен root, а у майнеров в HiveOS он есть. Без NVML или прав майнер пишет причину в лог и майнит как обычно.
- **Первый подбор занимает около 15 минут на карту** (10-25 в зависимости от числа шагов), и пока он идёт, риг не майнит (карты подбираются параллельно).
  Последующие запуски добавляют около 30 с. Результат хранится в `/hive-config/freeforgempear/uv.json` (переживает
  обновления майнера и перезагрузки); при смене драйвера, сборки ядра или начального offset подбор повторяется.
- Сначала задайте вкладку Overclock (фиксация ядра `1560`, память, лимит мощности). Подбор стартует с того offset, который
  стоит на карте в момент запуска майнера, и идёт только вверх; зафиксированные частоты, лимит мощности и память он не трогает.
  Если HiveOS заново применит разгон и изменит offset во время майнинга, auto-uv это заметит, отступит на этой карте и не будет воевать.
- Затрагиваются только карты, на которых работает майнер (сопоставление по PCI-адресу, он виден в логе). Каждое изменение
  пишется в лог так: `auto-uv: GPU0 (bus 0d, RTX 3070) offset +150 -> +195, 116.6 W -> 106.6 W, saved`; `grep auto-uv` по логу майнера покажет всё.
- При остановке майнера ваш offset возвращается. Если майнер был убит, при следующем запуске он восстанавливается первым делом.
- **Сброс:** один раз выполните `freeforgempear --uv-reset` в папке майнера (или удалите `/hive-config/freeforgempear/uv.json`).
  Остальные параметры (`--uv-offset N`, `--uv-max N`, `--uv-reval-hours H`, `--auto-uv-dry-run`) и порядок ручной настройки —
  в [MANUAL.md](MANUAL.md#12-андервольт---auto-uv-необязательно-новое-в-140). Андервольт — на ваш риск, как и любой разгон.

### Требования

- HiveOS с драйвером NVIDIA 525 или новее.
- Оптимизирован под RTX 30xx. RTX 40xx должны работать, но не проверены.
- Старые карты и AMD не поддерживаются.

### Комиссия разработчика

1%: 60 секунд из каждых 100 минут, на том же пуле, что указан у вас. Подробнее — в [LICENSE](LICENSE).
