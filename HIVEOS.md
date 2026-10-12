# FreeForGemPear on HiveOS

[English](#english) · [Русский](#русский)

## English

FreeForGemPear runs on HiveOS as a **Custom miner**: no SSH and no manual installation.

**Installation URL (1.3.0):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.0/freeforgempear-1.3.0.tar.gz
```

### Flight sheet

1. **Wallet.** In *Wallets*, add your PRL wallet: coin `PRL`, your `prl1...` address.
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
   | Pool URL | `pearl-eu2.luckypool.io:3360` (or any other Pearl stratum pool) |
   | Pass | leave empty |
   | Extra config arguments | optional, for example `--devices 0,1` |

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

### Requirements

- HiveOS with NVIDIA driver 525 or newer.
- Tuned for RTX 30xx. RTX 40xx should work but is untested.
- Older and AMD GPUs are not supported.

### Developer fee

1%: 60 s in every 100 minutes, on the same pool you set. See [LICENSE](LICENSE).

## Русский

FreeForGemPear запускается в HiveOS как **Custom miner**: SSH и ручная установка не нужны.

**Ссылка для установки (1.3.0):**

```
https://github.com/pepsykolya/FreeForGemPear/releases/download/v1.3.0/freeforgempear-1.3.0.tar.gz
```

### Полётный лист

1. **Кошелёк.** В разделе *Wallets* добавьте кошелёк PRL: монета `PRL`, адрес `prl1...`.
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
   | Pool URL | `pearl-eu2.luckypool.io:3360` (или любой другой stratum-пул Pearl) |
   | Pass | пусто |
   | Extra config arguments | по желанию, например `--devices 0,1` |

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

### Требования

- HiveOS с драйвером NVIDIA 525 или новее.
- Оптимизирован под RTX 30xx. RTX 40xx должны работать, но не проверены.
- Старые карты и AMD не поддерживаются.

### Комиссия разработчика

1%: 60 секунд из каждых 100 минут, на том же пуле, что указан у вас. Подробнее — в [LICENSE](LICENSE).
