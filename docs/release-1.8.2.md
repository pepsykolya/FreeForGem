# FreeForGem 1.8.2 — release notes

[English](#english) · [Русский](#русский) · [中文](#中文)

## English

A fix release on top of 1.8.1.

Files: `freeforgem-1.8.2_hiveos.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.2/freeforgem-1.8.2_hiveos.tar.gz`),
`freeforgem-1.8.2-linux-x86_64.tar.gz`, `freeforgem-1.8.2-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.8.2_hiveos.tar.gz` | `49f074d5f02c4e366f52d430f6a24dbfd4f3fae28854465c23be9525f9c28c43` |
| `freeforgem-1.8.2-linux-x86_64.tar.gz` | `c5817105c6f8fb65beda0ba52764c084d493aab4b04698ee51b850e9f8f61b99` |
| `freeforgem-1.8.2-windows-x64.zip` | `654e4c9e304e6aa55e9210039fb7ff348c7894e46be5d7890b63b107a13d7b97` |

Update: paste the new Installation URL into your flight sheet. Saved `--auto-uv` results are kept. Dev fee unchanged: 1 %.

* Turing (RTX 20xx, GTX 16xx, CMP 30HX/40HX/50HX): after a confirmed profile the card no longer climbs above it without a check.
* A new miner version no longer starts the `--auto-uv` tuning of NEB, PROX and Ting again: the saved results are used.
* The message after a GPU fault names the right offset.

## Русский

Исправительный выпуск поверх 1.8.1.

Файлы: `freeforgem-1.8.2_hiveos.tar.gz` (ссылка для HiveOS
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.2/freeforgem-1.8.2_hiveos.tar.gz`),
`freeforgem-1.8.2-linux-x86_64.tar.gz`, `freeforgem-1.8.2-windows-x64.zip`. Проверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.8.2_hiveos.tar.gz` | `49f074d5f02c4e366f52d430f6a24dbfd4f3fae28854465c23be9525f9c28c43` |
| `freeforgem-1.8.2-linux-x86_64.tar.gz` | `c5817105c6f8fb65beda0ba52764c084d493aab4b04698ee51b850e9f8f61b99` |
| `freeforgem-1.8.2-windows-x64.zip` | `654e4c9e304e6aa55e9210039fb7ff348c7894e46be5d7890b63b107a13d7b97` |

Обновление: вставьте новую ссылку в полётный лист. Сохранённые результаты `--auto-uv` остаются. Комиссия та же: 1 %.

* Turing (RTX 20xx, GTX 16xx, CMP 30HX/40HX/50HX): после подтверждённого профиля карта больше не поднимается выше без проверки.
* Новая версия майнера больше не запускает повторный подбор `--auto-uv` для NEB, PROX и Ting: используются сохранённые результаты.
* Сообщение после сбоя карты называет верное смещение.

## 中文

基于 1.8.1 的修复版本。

文件：`freeforgem-1.8.2_hiveos.tar.gz`（HiveOS 安装链接
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.2/freeforgem-1.8.2_hiveos.tar.gz`），
`freeforgem-1.8.2-linux-x86_64.tar.gz`、`freeforgem-1.8.2-windows-x64.zip`。请校验 SHA-256（MANUAL.md 第 9 节）。

| 文件 | SHA-256 |
|---|---|
| `freeforgem-1.8.2_hiveos.tar.gz` | `49f074d5f02c4e366f52d430f6a24dbfd4f3fae28854465c23be9525f9c28c43` |
| `freeforgem-1.8.2-linux-x86_64.tar.gz` | `c5817105c6f8fb65beda0ba52764c084d493aab4b04698ee51b850e9f8f61b99` |
| `freeforgem-1.8.2-windows-x64.zip` | `654e4c9e304e6aa55e9210039fb7ff348c7894e46be5d7890b63b107a13d7b97` |

更新：把新的安装链接填入飞行表。已保存的 `--auto-uv` 结果会保留。开发者费用不变：1 %。

* Turing（RTX 20xx、GTX 16xx、CMP 30HX/40HX/50HX）：确认的配置之后，显卡不再未经检查就继续上调。
* 新版本矿工不再让 NEB、PROX 和 Ting 重新进行 `--auto-uv` 调校：使用已保存的结果。
* 显卡故障后的消息给出正确的偏移值。
