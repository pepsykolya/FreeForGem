# FreeForGem 1.8.3 — release notes

[English](#english) · [Русский](#русский) · [中文](#中文)

## English

A NEB release on top of 1.8.2.

Files: `freeforgem-1.8.3_hiveos.tar.gz` (HiveOS Installation URL
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.3/freeforgem-1.8.3_hiveos.tar.gz`),
`freeforgem-1.8.3-linux-x86_64.tar.gz`, `freeforgem-1.8.3-windows-x64.zip`. Check the SHA-256 (MANUAL.md, section 9).

| File | SHA-256 |
|---|---|
| `freeforgem-1.8.3_hiveos.tar.gz` | `4f8c6e21058ca6da6a99ccd2fda22cee6559057bb957d91e1b42b17aae6c95f0` |
| `freeforgem-1.8.3-linux-x86_64.tar.gz` | `75cf8313539d2da61c1ad9ad9f5059da40cd95b487455b7b8f70458fc11ff3a4` |
| `freeforgem-1.8.3-windows-x64.zip` | `f9357cd8031f42f0c0c1fac586fae260699d331507fc6e726721b8eee524b13c` |

Update: paste the new Installation URL into your flight sheet. Saved `--auto-uv` results of 1.8.1 and 1.8.2 are kept: no new
tuning after the update. Dev fee unchanged: 1 %.

* NEB earns more in its slow blocks: on RTX 30 cards such blocks give 5–27 % more per watt (measured on an RTX 3070).
* New, optional: a NEB card can mine another coin of this miner while NEB is slow, and comes back to NEB when it is fast
  again. Turn it on with `--neb-slow-alt pearl` (or `noid`, `prox`, `ting`, `cmfd`) plus `--alt-pool` and `--alt-wallet`
  for that coin. The card switches only when the other coin pays more per watt. The other coin keeps its own pool, its own
  saved `--auto-uv` profile (it is not tuned in these short windows) and its own 1 % dev fee. A switch takes about 10 s (CMFD: about 30 s). Without the option NEB
  works as before.
* HiveOS: do not use `--alt-args` with a value that contains spaces (HiveOS drops the quotes and the miner stops with "unknown argument"); without it the other coin takes the clock locks of the HiveOS OC profile. One flag without a value works: `--alt-args --flag`.
* The table shows such a card as `pearl (slow NEB model)`. HiveOS shows its hash rate on the other coin.
* If the connection to the pool is lost, the miner notices it in about half a minute and reconnects; shares that are still valid are sent again.
* The NEB engine in the packages contains no local paths of the build machine.

## Русский

Выпуск для NEB поверх 1.8.2.

Файлы: `freeforgem-1.8.3_hiveos.tar.gz` (ссылка для HiveOS
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.3/freeforgem-1.8.3_hiveos.tar.gz`),
`freeforgem-1.8.3-linux-x86_64.tar.gz`, `freeforgem-1.8.3-windows-x64.zip`. Проверьте SHA-256 (MANUAL.md, раздел 9).

| Файл | SHA-256 |
|---|---|
| `freeforgem-1.8.3_hiveos.tar.gz` | `4f8c6e21058ca6da6a99ccd2fda22cee6559057bb957d91e1b42b17aae6c95f0` |
| `freeforgem-1.8.3-linux-x86_64.tar.gz` | `75cf8313539d2da61c1ad9ad9f5059da40cd95b487455b7b8f70458fc11ff3a4` |
| `freeforgem-1.8.3-windows-x64.zip` | `f9357cd8031f42f0c0c1fac586fae260699d331507fc6e726721b8eee524b13c` |

Обновление: вставьте новую ссылку в полётный лист. Сохранённые результаты `--auto-uv` версий 1.8.1 и 1.8.2 остаются: после
обновления повторного подбора нет. Комиссия та же: 1 %.

* NEB больше зарабатывает в медленных блоках: на картах RTX 30 такие блоки дают на 5–27 % больше на ватт (замер на RTX 3070).
* Новое, по желанию: карта NEB может майнить другую монету этого майнера, пока NEB медленный, и возвращается на NEB, когда он
  снова быстрый. Включается так: `--neb-slow-alt pearl` (или `noid`, `prox`, `ting`, `cmfd`), плюс `--alt-pool` и
  `--alt-wallet` для этой монеты. Карта переключается, только если другая монета платит больше на ватт. У другой монеты свой
  пул, свой сохранённый профиль `--auto-uv` (в этих коротких окнах он не подбирается) и своя комиссия 1 %. Переключение занимает около 10 с (CMFD — около 30 с). Без
  этой опции NEB работает как раньше.
* HiveOS: не используйте `--alt-args` со значением, в котором есть пробелы (HiveOS теряет кавычки, и майнер останавливается с «unknown argument»); без него другая монета берёт локи частот из OC-профиля HiveOS. Один флаг без значения работает: `--alt-args --флаг`.
* В таблице такая карта видна как `pearl (slow NEB model)`. HiveOS показывает её хешрейт на другой монете.
* Если связь с пулом пропала, майнер замечает это примерно за полминуты и переподключается; ещё действительные шары отправляются заново.
* В движке NEB из пакетов нет локальных путей машины сборки.

## 中文

基于 1.8.2 的 NEB 版本。

文件：`freeforgem-1.8.3_hiveos.tar.gz`（HiveOS 安装链接
`https://github.com/pepsykolya/FreeForGem/releases/download/v1.8.3/freeforgem-1.8.3_hiveos.tar.gz`），
`freeforgem-1.8.3-linux-x86_64.tar.gz`、`freeforgem-1.8.3-windows-x64.zip`。请校验 SHA-256（MANUAL.md 第 9 节）。

| 文件 | SHA-256 |
|---|---|
| `freeforgem-1.8.3_hiveos.tar.gz` | `4f8c6e21058ca6da6a99ccd2fda22cee6559057bb957d91e1b42b17aae6c95f0` |
| `freeforgem-1.8.3-linux-x86_64.tar.gz` | `75cf8313539d2da61c1ad9ad9f5059da40cd95b487455b7b8f70458fc11ff3a4` |
| `freeforgem-1.8.3-windows-x64.zip` | `f9357cd8031f42f0c0c1fac586fae260699d331507fc6e726721b8eee524b13c` |

更新：把新的安装链接填入飞行表。1.8.1 和 1.8.2 已保存的 `--auto-uv` 结果会保留：更新后不会重新调校。开发者费用不变：1 %。

* NEB 在慢区块中收益更高：在 RTX 30 显卡上，这类区块每瓦多 5–27 %（在 RTX 3070 上测得）。
* 新功能（可选）：NEB 变慢时，NEB 显卡可以改挖本矿工支持的另一种币，NEB 恢复变快后再回到 NEB。启用方式：
  `--neb-slow-alt pearl`（或 `noid`、`prox`、`ting`、`cmfd`），再加上该币的 `--alt-pool` 和 `--alt-wallet`。只有当另一种币
  每瓦收益更高时显卡才会切换。另一种币使用自己的矿池、自己已保存的 `--auto-uv` 配置（在这些短窗口中不进行调校）和自己的 1 % 开发者费用。一次切换约
  10 秒（CMFD 约 30 秒）。不加此选项时 NEB 与之前相同。
* HiveOS：不要使用带空格值的 `--alt-args`（HiveOS 会去掉引号，矿工会因 "unknown argument" 停止）；不加它时，另一种币使用 HiveOS 超频配置中的频率锁定。不带值的单个标志可以使用：`--alt-args --标志`。
* 表格中这样的显卡显示为 `pearl (slow NEB model)`。HiveOS 显示它在另一种币上的算力。
* 如果与矿池的连接中断，矿工大约半分钟内就会发现并重新连接；仍然有效的份额会重新提交。
* 安装包中的 NEB 引擎不再包含构建机器的本地路径。
