# ProtoSwitch

**ProtoSwitch v0.2.0-beta.1** — terminal-first утилита для Telegram Desktop, которая следит за состоянием proxy, подбирает замену из бесплатных MTProto/SOCKS5-источников и тихо пишет новый managed proxy в настройки Telegram без popup и без focus stealing.

## Что Есть Сейчас

- watcher для фоновой проверки и ротации proxy;
- adaptive TUI с режимами `Обзор`, `Команды`, `Источники`, `История`;
- managed backend для `tdata/settingss`, чтобы не засорять Telegram случайными нерабочими адресами;
- manual fallback для явного `switch` и `repair`, если нужен live-сценарий;
- structured UTF-8 логи без старого `String::from_utf8_lossy`-хаоса;
- Windows installer + portable-артефакты для Windows, Linux и macOS;
- автозапуск через `Scheduled Task` / `Startup folder` на Windows, XDG autostart на Linux и LaunchAgent на macOS.

## Артефакты

| Файл | Назначение |
| --- | --- |
| `ProtoSwitch-Setup-x64.exe` | обычная установка для Windows x64 |
| `protoswitch-portable-win-x64.zip` | portable для Windows x64 |
| `protoswitch-portable-linux-x64.tar.gz` | portable для Linux x64 |
| `protoswitch-portable-linux-arm64.tar.gz` | portable для Linux arm64 |
| `protoswitch-portable-macos-x64.tar.gz` | portable для macOS x64 |
| `protoswitch-portable-macos-arm64.tar.gz` | portable для macOS arm64 |

Windows installer остаётся только для Windows. Linux и macOS в этой очереди идут как portable-first beta.

## Как Работает

```mermaid
flowchart TD
    A["watch / switch"] --> B["Проверить текущий proxy"]
    B -->|ok| C["Обновить state"]
    B -->|fail| D["Взять кандидата"]
    D --> E["Локально проверить"]
    E -->|bad| D
    E -->|good| F{"Telegram открыт?"}
    F -->|нет| G["Тихо записать managed proxy"]
    F -->|да| H["Тихо записать managed proxy<br/>и пометить waiting_for_restart"]
    G --> I["Proxy применится при следующем запуске Telegram"]
    H --> I
```

Фоновый watcher не должен поднимать Telegram поверх других окон. Если клиент уже открыт, ProtoSwitch сохраняет новый proxy в managed subset и честно показывает, что он ждёт следующего запуска Telegram.

## Быстрый Старт

### Windows

1. Установите `ProtoSwitch-Setup-x64.exe` или распакуйте `protoswitch-portable-win-x64.zip`.
2. Запустите `protoswitch.exe` без аргументов.
3. Проверьте состояние:
   `protoswitch status --plain`
   `protoswitch doctor`
4. При необходимости включите автозапуск:
   `protoswitch autostart install`
5. Для ручной смены proxy:
   `protoswitch switch`

### Linux / macOS

1. Распакуйте portable-архив под свою архитектуру.
2. Запустите:
   `./protoswitch init --non-interactive --no-autostart`
3. Проверьте состояние:
   `./protoswitch status --plain`
   `./protoswitch doctor`
4. Для фоновой работы:
   `./protoswitch watch --headless`

## Основные Команды

| Команда | Что делает |
| --- | --- |
| `protoswitch init` | создаёт или обновляет `config.toml` |
| `protoswitch status` | показывает текущее состояние proxy, backend и автозапуска |
| `protoswitch watch` | запускает watcher |
| `protoswitch switch` | сразу ищет и применяет новый proxy |
| `protoswitch cleanup` | чистит dead ProtoSwitch-owned proxy из managed subset |
| `protoswitch doctor` | проводит диагностику окружения |
| `protoswitch repair` | восстанавливает локальную установку |
| `protoswitch shutdown` | полностью останавливает процессы ProtoSwitch |
| `protoswitch autostart install` | включает автозапуск |
| `protoswitch autostart remove` | выключает автозапуск |

## Конфиг И Данные

В `config.toml` закреплён блок:

```toml
[telegram]
client = "desktop"
backend_mode = "hybrid"
data_dir = ""
```

`backend_mode`:

- `managed` — только тихая запись в `settingss`;
- `hybrid` — managed path по умолчанию, live fallback только для явных ручных действий;
- `manual` — без фонового live-apply watcher всё равно остаётся silent-only.

Каталоги данных:

| ОС | Конфиг | State / logs | Автозапуск |
| --- | --- | --- | --- |
| Windows | `%APPDATA%\ProtoSwitch\config.toml` | `%LOCALAPPDATA%\ProtoSwitch\state.json`, `%LOCALAPPDATA%\ProtoSwitch\logs\watch.log` | `Scheduled Task` или `Startup folder` |
| Linux | XDG config dir | XDG data dir | `~/.config/autostart/protoswitch.desktop` |
| macOS | `~/Library/Application Support/ProtoSwitch` | `~/Library/Application Support/ProtoSwitch` | `~/Library/LaunchAgents/com.thelitis.protoswitch.plist` |

## Источники Proxy

По умолчанию ProtoSwitch использует пул из нескольких бесплатных лент:

- `mtproto.ru`
- `SoliSpirit/mtproto`
- `Argh94/Proxy-List` для `MTProto`
- `Argh94/Proxy-List` для `SOCKS5`
- `proxifly/free-proxy-list`
- `hookzof/socks5_list`

Новый кандидат сначала проходит локальную проверку, и только потом попадает в managed subset Telegram.

## Ограничения Beta

- поддерживается только `Telegram Desktop`;
- Linux/macOS сейчас portable-first и всё ещё считаются beta-grade веткой;
- фоновый watcher не делает true live-switch для уже открытого Telegram, а использует честную схему `silent save + next launch`;
- бесплатные proxy по природе нестабильны, поэтому это всё ещё beta, а не stable.
