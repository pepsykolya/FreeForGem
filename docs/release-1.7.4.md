# FreeForGem 1.7.4 — release notes

[English](#english) · [Русский](#русский)

## English

Files: `freeforgem-1.7.4.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.4/freeforgem-1.7.4.tar.gz`),
`freeforgem-1.7.4-linux-x86_64.tar.gz`, `freeforgem-1.7.4-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.7.4.tar.gz` | `65db39b90514b1080dbc0f628d76f6cd577b3f1f2dbca5881daf06aac08fd240` |
| `freeforgem-1.7.4-linux-x86_64.tar.gz` | `0ce860c49987c8e1b1da59f14beac667a08af9e3ff036e8765b0d46f8c3ff2ae` |
| `freeforgem-1.7.4-windows-x64.zip` | `f98065c6559f5ab18f98fa923155aa616f409cb93481808d67149ea0776d1947` |


**New: Kryptex account username as the wallet** (Pearl). Kryptex pays in BTC/USDT with auto-conversion when you mine to your
Kryptex username instead of a `prl1...` address. 1.7.4 logs in as `username/WORKER` there (Kryptex's format), and the Windows
start file accepts a username on a Kryptex pool. 1.7.3 and older stopped on Windows with "WALLET does not look like a PRL address".

* Windows: put your Kryptex username into `WALLET` of a `start-kryptex-*.bat`.
* HiveOS: the username as the wallet, the template stays `%WAL%.%WORKER_NAME%`.
* Command line: `--pool prl-eu.kryptex.network:7048 --wallet YOUR_KRYPTEX_USERNAME --worker rig1`.

Test (RTX 3070, Kryptex Europe, username login): authorized, shares accepted, 0 rejected. Everything else is as in 1.7.3.

## Русский

Файлы: `freeforgem-1.7.4.tar.gz` (ссылка для установки HiveOS
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.7.4/freeforgem-1.7.4.tar.gz`),
`freeforgem-1.7.4-linux-x86_64.tar.gz`, `freeforgem-1.7.4-windows-x64.zip`. Сверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.7.4.tar.gz` | `65db39b90514b1080dbc0f628d76f6cd577b3f1f2dbca5881daf06aac08fd240` |
| `freeforgem-1.7.4-linux-x86_64.tar.gz` | `0ce860c49987c8e1b1da59f14beac667a08af9e3ff036e8765b0d46f8c3ff2ae` |
| `freeforgem-1.7.4-windows-x64.zip` | `f98065c6559f5ab18f98fa923155aa616f409cb93481808d67149ea0776d1947` |


**Новое: имя пользователя Kryptex вместо кошелька** (Pearl). Если майнить на имя пользователя Kryptex, а не на адрес `prl1...`,
Kryptex платит в BTC/USDT с автоконвертацией. 1.7.4 входит туда как `имя/ВОРКЕР` (формат Kryptex), а стартовый файл Windows
пропускает имя пользователя на пулах Kryptex. 1.7.3 и старше на Windows останавливались с «WALLET does not look like a PRL address».

* Windows: впишите имя пользователя Kryptex в `WALLET` файла `start-kryptex-*.bat`.
* HiveOS: имя пользователя как кошелёк, шаблон по-прежнему `%WAL%.%WORKER_NAME%`.
* Командная строка: `--pool prl-eu.kryptex.network:7048 --wallet ВАШЕ_ИМЯ_KRYPTEX --worker rig1`.

Тест (RTX 3070, Kryptex Европа, вход по имени пользователя): вход принят, шары принимаются, 0 отклонено. Остальное — как в 1.7.3.
