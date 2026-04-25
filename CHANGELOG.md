# Changelog

Все заметные изменения проекта ProtoSwitch будут отражаться в этом файле.

## [Unreleased]

### Changed

- CI и release workflows переведены на актуальные `actions/upload-artifact@v7` и `actions/download-artifact@v8`, чтобы убрать Node.js runtime-аннотацию без workaround env в следующих релизных запусках.

## [v0.2.0-beta.6] - 2026-04-25

Hotfix-beta после `v0.2.0-beta.5`: закрывает flaky Windows e2e на локальном provider fixture и убирает предупреждения GitHub Actions про Node.js runtime.

### Changed

- Link-list и SOCKS provider fetch теперь используют общий retry-контур `fetch_attempts` / `fetch_retry_delay_ms`, поэтому кратковременный сетевой сбой не срывает источник с первой попытки.
- Детерминированный watcher e2e стал устойчивее к race при старте локального HTTP fixture на Windows CI.
- Windows installer smoke больше не зависит от live `mtproto.ru`: после установки он отключает публичные provider sources во временном конфиге.
- CI и release workflows явно включают `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true`.

### Fixed

- Исправлен красный `main` CI, где Windows test иногда падал на `watcher_e2e_writes_managed_settings_when_telegram_is_open` из-за слишком раннего запроса к fixture server.

## [v0.2.0-beta.5] - 2026-04-25

Beta-релиз для доводки поставки после `v0.2.0-beta.4`: installer теперь открывает пользовательский сценарий через системный индикатор по умолчанию, а репозиторий получил отдельный push/PR CI, который проверяет Windows, Linux и macOS до релизной сборки.

### Changed

- Ярлык `ProtoSwitch` в меню Пуск, desktop shortcut и финальный запуск installer теперь стартуют `protoswitch tray`, чтобы пользователь сразу видел индикатор в системной области, а не отдельную консоль.
- Для продвинутого сценария в меню Пуск оставлен отдельный `ProtoSwitch Console`.
- Windows installer smoke теперь проверяет, что основной ярлык действительно запускает tray-mode.
- Release workflow переведён на актуальные `actions/checkout@v5`, `actions/upload-artifact@v5` и `actions/download-artifact@v5`, чтобы убрать предупреждения о старом Node.js runtime.
- Добавлен отдельный CI workflow на push и pull request: форматирование, тесты и `clippy -D warnings` проходят на Windows, Linux и macOS.

### Fixed

- Убрано дублирование `allow(dead_code)`, которое ломало non-Windows `clippy` при строгих предупреждениях.

## [v0.2.0-beta.4] - 2026-04-24

Beta-релиз для пользовательского фонового сценария: ProtoSwitch получил системный индикатор, автозапуск теперь поднимает tray-mode, а watcher при открытом Telegram продолжает managed rotation без popup, без захвата фокуса и без состояния `waiting_for_restart`.

### Added

- Добавлена команда `protoswitch tray` с индикатором в системной области Windows/Linux и menu bar на macOS.
- В tray-menu добавлены быстрые действия: найти новый proxy, перезапустить watcher, остановить ProtoSwitch и скрыть индикатор.

### Changed

- Автозапуск теперь стартует `protoswitch tray`, чтобы у пользователя был видимый фоновый индикатор.
- Managed apply при открытом Telegram больше не переводит состояние в `waiting_for_restart`: ProtoSwitch записывает proxy в `settingss`, включает Telegram proxy rotation и продолжает watcher-цикл.
- Первый запуск watcher теперь сам ищет и записывает рабочий proxy, а не ждёт ручной `switch`.
- Ручной `switch` больше не открывает `tg://` fallback: команда работает через managed settings, как и фоновый watcher.
- Главный экран TUI стал короче: убраны спорные формулировки и отдельная перегружающая панель.
- Документация и quickstart обновлены под tray-mode и managed rotation.

### Notes

- Tray на Linux зависит от поддержки appindicator/tray в конкретном desktop environment.
- Live-переключение не использует `tg://`-диалог и не нажимает кнопки в Telegram; фоновый путь работает через managed settings и Telegram proxy rotation.

## [v0.2.0-beta.3] - 2026-04-23

Beta-релиз для закрытия watcher e2e-хвоста: ProtoSwitch теперь имеет детерминированный sandbox e2e-набор для managed watcher-flow, отдельный opt-in live Windows smoke для реального Telegram Desktop и release-gate, который не даёт упаковке Windows пройти без зелёного watcher e2e.

### Added

- Добавлен internal test seam для детерминированного состояния `Telegram запущен / не запущен` внутри crate-level e2e.
- Добавлены sandbox helper-ы для реального roundtrip `tdata/settingss` через текущий `tdesktop` serializer.
- Добавлен crate-level watcher e2e suite с локальными HTTP/TCP fixture-серверами и проверками `state.json`, `doctor --json`, `status --json` и managed `settingss`.
- Добавлен `scripts/e2e-windows-live.ps1` для opt-in live smoke на реальном Windows + Telegram Desktop с backup/restore исходного `settingss`.

### Changed

- Watcher e2e теперь покрывает сценарии `healthy current proxy`, `pending when Telegram closed`, `managed apply when Telegram open`, `dead first candidate / live second candidate` и `source empty / no free proxies`.
- CI release workflow блокирует Windows packaging на зелёном deterministic watcher e2e job.
- `tdesktop` data-dir override стал терпимее к sandbox путям и legacy `settings`, чтобы e2e мог валидировать реальный бинарный формат без подмены файлового layout.
- Release guide, quickstart и README обновлены под `v0.2.0-beta.3` и новый e2e-контур.

### Notes

- Критерий успеха для уже открытого Telegram не изменился: watcher по-прежнему считает фоновым успехом только запись в managed settings до следующего запуска клиента, а не true live-switch.
- Live Windows smoke остаётся локальным opt-in сценарием и не меняет CI-контракт для Linux/macOS.

## [v0.2.0-beta.2] - 2026-04-23

Beta-релиз hardening-очереди: Windows-friendly UTF-8 доведён до практического состояния для PowerShell и installer-текстов, managed backend стал строже различать `active / saved / waiting_for_restart / source empty / manual fallback unavailable`, а Linux/macOS portable-ветка получила реальный smoke-слой в CI вместо compile-only отношения.

### Added

- Добавлен `scripts/smoke-unix.py` для portable smoke на Linux/macOS с проверкой `init`, `status`, `doctor`, `autostart install/remove` и OS-specific путей.
- В unit-тесты добавлена загрузка `config.toml` и `state.json` из UTF-8 BOM файлов.
- В TUI-тесты добавлена отдельная проверка переноса длинных русских статусов.

### Changed

- Загрузка `config.toml` и `state.json` теперь идёт через единый decode-слой, а не через прямой `read_to_string`, поэтому старые BOM/NUL/cp1251/ibm866 кейсы реже ломают runtime.
- Background apply больше не превращает неудачный manual fallback в ложный hard error: managed settings сохраняются, а статус честно показывает `ручной fallback недоступен`.
- Plain status, doctor и watcher-статусы стали строже различать `активен`, `ждёт перезапуска Telegram`, `источник пуст`, `кандидат отклонён локальной проверкой` и `источник недоступен`.
- Responsive TUI теперь аккуратнее переносит длинные значения в `Dashboard`, `Providers` и `History` вместо агрессивного middle-ellipsis.
- Windows smoke-скрипты проверяют отсутствие mojibake в plain output и bundled docs.
- GitHub Actions release matrix теперь не только собирает Linux/macOS portable-артефакты, но и реально прогоняет их smoke-сценарии; все build jobs идут с `RUSTFLAGS=-Dwarnings`.
- README, CHANGELOG, quickstart-файлы и installer-текст подготовлены к BOM-safe UTF-8 потоку для Windows PowerShell.

### Notes

- Контракт для уже открытого Telegram не изменился: watcher по-прежнему использует managed settings до следующего запуска клиента.
- Linux/macOS остаются portable-first beta-веткой, но теперь это уже smoke-проверяемая ветка, а не просто compile-only упаковка.

## [v0.2.0-beta.1] - 2026-04-23

Beta-релиз, который переводит ProtoSwitch из чисто Windows-first прототипа в более широкую multi-platform beta-ветку: watcher теперь использует managed settings для уже открытого Telegram, TUI стал адаптивным и заметно спокойнее на узких окнах, release flow разделён на Windows installer и portable-артефакты для Windows, Linux и macOS, а GitHub Actions собирает и публикует все эти варианты автоматически.

### Added

- В `config.toml` закреплён блок `[telegram]` с `client`, `backend_mode` и опциональным `data_dir`.
- Добавлен platform-layer для Linux/macOS с OS-native автозапуском:
  - Linux: XDG autostart `.desktop`
  - macOS: `LaunchAgent`
- Добавлен multi-platform portable packaging и release matrix в `.github/workflows/release.yml`.
- В релизную очередь теперь входят 6 артефактов:
  - `ProtoSwitch-Setup-x64.exe`
  - `protoswitch-portable-win-x64.zip`
  - `protoswitch-portable-linux-x64.tar.gz`
  - `protoswitch-portable-linux-arm64.tar.gz`
  - `protoswitch-portable-macos-x64.tar.gz`
  - `protoswitch-portable-macos-arm64.tar.gz`

### Changed

- Фоновый watcher больше не использует live-popup как основной путь: он пишет ProtoSwitch-managed proxy в `settingss` и, если Telegram уже открыт, помечает состояние как ожидающее следующего запуска клиента.
- Ручные `switch` и `repair` остаются `hybrid`: managed-path идёт первым, а live fallback остаётся только явным пользовательским сценарием.
- TUI перестроен в `narrow / regular / wide` режимы, получил semantic colors, прокрутку сигналов через `PgUp/PgDn` и меньше агрессивного middle-ellipsis на статусных карточках.
- `doctor` и plain status теперь отдельно показывают состояние источника, backend apply, путь применения, платформу и признак `waiting_for_restart`.
- Decode-слой для внешних процессов и structured UTF-8 log writing усилены, чтобы ошибки PowerShell/Windows реже превращались в битый текст.
- Windows distribution scripts переведены на общую portable packaging-схему, а manual publish script теперь работает со всеми файлами из `dist/<version>`.

### Notes

- Windows installer по-прежнему существует только для Windows x64; Linux и macOS в этой очереди идут как portable-first beta.
- Для уже открытого Telegram фоновый успех — это `saved to managed settings`, а не мгновенный live-switch.

## [v0.1.0-beta.11] - 2026-04-23

Бета-релиз для полировки интерфейса и безопасного повседневного запуска: ProtoSwitch больше не пытается навязчиво применять stale `pending proxy`, `doctor` в TUI выполняется в фоне, installer получил отдельную точку входа для починки, а README переписан в более чистом пользовательском виде.

### Changed

- Обычный запуск больше не должен внезапно трогать внешний proxy, если у ProtoSwitch ещё нет текущего управляемого адреса.
- Если текущий proxy жив, stale `pending proxy` автоматически сбрасывается и не провоцирует лишний popup Telegram.
- `doctor` в консольном интерфейсе вынесен в фоновую задачу, поэтому UI не должен подвисать на время диагностики.
- Карточки и сигналы в TUI стали компактнее и лучше переживают узкие окна.
- В installer добавлен отдельный ярлык `Починить ProtoSwitch`.
- README сокращён и переписан без внутренних release-операторских деталей.

### Notes

- Этот релиз продолжает ветку `beta`, а не объявляет stable: главный риск по-прежнему находится в Telegram UI automation и качестве бесплатных proxy.

## [v0.1.0-beta.10] - 2026-04-23

Бета-релиз для доводки именно runtime-переключения proxy: ProtoSwitch теперь не считает proxy применённым, пока Telegram не подтвердит его как рабочий, проверяет кандидат в proxy-диалоге до добавления в список и автоматически перебирает следующие адреса, если первый был отвергнут.

### Changed

- Apply flow в Telegram теперь проходит по схеме `Check Status -> Add/Connect -> settle`, если в proxy-диалоге доступна кнопка проверки статуса.
- ProtoSwitch считает успешным применением только финальный статус `Available`; состояния `Checking`, `Unknown` и `Missing` больше не принимаются за рабочий proxy.
- Если Telegram отклоняет proxy, не подтверждает его или не сохраняет в списке, приложение автоматически убирает такой кандидат из управляемого списка и пробует следующий.
- `watch` и `switch` теперь умеют проходить по нескольким кандидатам подряд, не застревая на одном и том же неудачном адресе после первого отказа.

### Notes

- Предварительная проверка перед добавлением использует UI Telegram, а не только локальный TCP health-check.
- Автоприменение по-прежнему работает только в той же интерактивной Windows-сессии, где запущен Telegram Desktop.

## [v0.1.0-beta.9] - 2026-04-22

Релиз для доводки продукта до более зрелого daily-driver состояния: ProtoSwitch получил новый dashboard с разнесёнными view, встроенный page для provider pool, поддержку бесплатных SOCKS5-источников как fallback и более реалистичную схему подбора рабочих кандидатов из нескольких постоянно обновляемых лент.

### Changed

- TUI полностью переложен в более собранный multi-view интерфейс `Dashboard / Actions / Providers / History` с gauges, более явными цветовыми акцентами и отдельной страницей провайдеров вместо единой перегруженной панели.
- Экран настройки теперь сразу показывает provider pool, включённые источники и отдельный переключатель `SOCKS5 fallback`.
- Встроенный provider pool расширен до нескольких внешних источников:
  - `mtproto.ru`
  - `SoliSpirit/mtproto`
  - `Argh94/Proxy-List` для `MTProto`
  - `Argh94/Proxy-List` для `SOCKS5`
  - `proxifly/free-proxy-list` для `SOCKS5`
  - `hookzof/socks5_list`
- `status`, `doctor` и TUI теперь показывают не один `source_url`, а полный активный пул источников и текущее состояние fallback-режима.
- Для бесплатных источников добавлена поддержка both `tg://proxy`, `https://t.me/proxy`, `tg://socks` и сырых `socks5://` / `host:port` списков.

### Notes

- Бесплатные proxy по природе нестабильны, поэтому ProtoSwitch по-прежнему проверяет кандидатов локальным TCP/SOCKS5 health-check до применения.
- SOCKS5 fallback используется как запасной слой, а не как обязательная замена MTProto: его можно отключить в TUI или в конфиге.

## [v0.1.0-beta.8] - 2026-04-22

Release для доводки повседневного Windows-сценария: интерфейс стал спокойнее и полезнее, ProtoSwitch начал валидировать новые proxy до применения, Telegram automation перестал так агрессивно лезть в фокус, а installer получил финальную галочку desktop shortcut и прозрачную иконку.

### Changed

- Обычный запуск теперь открывает более спокойную terminal-first console вместо перегруженного dashboard в духе старых file manager layout.
- В интерфейсе и в `protoswitch status` появились явные статусы текущего proxy и источника: работает, нет данных, replacement не найден, есть pending proxy и другие рабочие состояния.
- `protoswitch cleanup` удаляет из Telegram мёртвые proxy, которыми ProtoSwitch управлял раньше, а после успешного применения нового proxy приложение пытается делать такую очистку автоматически.
- Новые кандидаты от `mtproto.ru` теперь проходят локальную TCP-проверку до применения, поэтому `doctor` и `switch` больше не считают любой найденный `tg://proxy` заведомо рабочим.
- Telegram apply flow теперь старается вернуть фокус в предыдущее окно после подтверждения `tg://proxy`, чтобы меньше мешать пользователю.
- Installer на финальной странице предлагает отдельную галочку добавления ярлыка ProtoSwitch на рабочий стол.
- PNG/ICO иконки очищены от запечённого светлого checkerboard-фона и теперь реально прозрачные.

### Notes

- Проверка proxy в этой версии всё ещё best-effort и основана на TCP-доступности, а не на полной эмуляции MTProto-сессии.
- Desktop shortcut создаётся только если пользователь оставил галочку включённой на финальной странице installer.

## [v0.1.0-beta.7] - 2026-04-22

UX-релиз для повседневной работы: ProtoSwitch получил новый операторский dashboard, app-like fallback в Startup folder и встроенную иконку для `exe` и installer.

### Changed

- Обычный запуск ProtoSwitch теперь открывает более сильный TUI-интерфейс с командами `switch`, `doctor`, `settings`, `autostart`, `watcher`, логами и быстрым доступом к папке данных.
- Экран настройки первого запуска переработан в более плотный и понятный конфигуратор вместо минимального списка из пяти строк.
- Startup fallback больше не создаёт `ProtoSwitch.cmd`: теперь используется `ProtoSwitch.lnk` с иконкой приложения.
- При обновлении с прошлых beta-версий legacy `ProtoSwitch.cmd` автоматически мигрируется в новый `.lnk` на первом запуске.
- В `protoswitch.exe` теперь вшита собственная Windows-иконка, и Inno Setup использует тот же `.ico` для installer.

### Notes

- Новый dashboard остаётся keyboard-first: все основные действия доступны горячими клавишами без выхода в отдельные команды.
- Если в системе удаётся создать Scheduled Task, он по-прежнему имеет приоритет над Startup folder.

## [v0.1.0-beta.6] - 2026-04-22

Bugfix-релиз для действительно автономной смены proxy: ProtoSwitch теперь дольше ждёт выдачу сервера на `mtproto.ru`, различает состояние `свободных серверов нет` и сам подтверждает диалог подключения в Telegram через Windows UI Automation.

### Changed

- Provider `mtproto.ru` теперь держит cookie jar, повторяет запросы серией попыток и не срывается после четырёх коротких retry.
- Если `mtproto.ru` временно не отдаёт proxy, ProtoSwitch показывает понятную причину `свободных серверов нет` вместо общего `не найден tg://proxy`.
- После открытия `tg://proxy?...` приложение ищет модальное окно Telegram и вызывает кнопку подключения без ручного клика.
- Если окно Telegram не удалось подтвердить автоматически, ProtoSwitch больше не помечает proxy как применённый молча.

### Notes

- Автоподтверждение работает в той же интерактивной Windows-сессии, где запущен Telegram Desktop.
- Если у `mtproto.ru` реально нет свободных серверов, watcher теперь оставляет эту причину в диагностике до следующей удачной попытки.

## [v0.1.0-beta.5] - 2026-04-22

Bugfix-релиз для рабочего Windows-сценария: приложение само поднимает watcher при обычном запуске, status-экран обновляется live, а применение `tg://proxy` больше не ломается на `&port=...&secret=...`.

### Changed

- Обычный запуск ProtoSwitch теперь поднимает headless watcher, если он еще не работает.
- Status-экран теперь обновляет состояние live вместо одного статичного снимка.
- Применение MTProto proxy переведено на PowerShell `Start-Process`, чтобы Windows корректно открывал `tg://proxy?...` с параметрами `server`, `port` и `secret`.

### Notes

- Этот релиз исправляет основной functional blocker `v0.1.0-beta.4`, при котором watcher мог запуститься, но не мог применить proxy в Telegram.

## [v0.1.0-beta.4] - 2026-04-22

Bugfix-релиз для Windows installer и ярлыка запуска: приложение больше не закрывается сразу при открытии без аргументов.

### Changed

- Запуск `protoswitch.exe` без аргументов теперь открывает пользовательский сценарий вместо мгновенного выхода с `help`.
- При первом запуске без готового конфига открывается экран настройки.
- После инициализации запуск без аргументов открывает экран статуса ProtoSwitch.
- README, QUICKSTART и release guide обновлены под `v0.1.0-beta.4`.

### Notes

- Этот релиз исправляет основной UX-дефект `v0.1.0-beta.3`, из-за которого double-click по приложению выглядел как "ничего не произошло".

## [v0.1.0-beta.3] - 2026-04-22

Первая Windows-distribution поставка проекта: portable-архив, Inno Setup installer, release-скрипты и ручной релизный процесс через GitHub Releases.

### Added

- Добавлены packaging-скрипты для сборки `ProtoSwitch-Setup-x64.exe` и `protoswitch-portable-win-x64.zip`.
- Описаны релизные артефакты `ProtoSwitch-Setup-x64.exe` и `protoswitch-portable-win-x64.zip`.
- Добавлена инструкция по установке через installer с выбором `только для текущего пользователя` или `для всех пользователей`.
- Добавлено описание portable-сценария без installer.
- Добавлен отдельный `RELEASE-GUIDE.md` с ручным процессом публикации через GitHub Releases.

### Changed

- README обновлен до `v0.1.0-beta.3`.
- Installer теперь сам выполняет первичную инициализацию и при включенном чекбоксе автозапуска вызывает ту же логику `scheduled_task` -> `startup_folder`, что и CLI.
- Уточнен ручной сценарий обновления через GitHub Releases для installer- и portable-версии.
- Уточнено поведение uninstall: uninstaller сам снимает `scheduled_task` или `startup_folder`, удаляет файлы поставки и сохраняет пользовательские `config/state/logs` до ручной очистки.
- Явно описано различие между per-user и machine-wide установкой.

### Notes

- Machine-wide install в этой очереди не делает ProtoSwitch системной службой.
- Автозапуск даже после machine-wide установки остается пользовательским для текущего установщика.
- Первая публичная поставка остается Windows-only и `x64`-only.

## [v0.1.0-beta.2] - 2026-04-22

Бета-обновление документации и пользовательского сценария после доработки fallback-механизма автозапуска.

### Changed

- Зафиксировано, что `autostart install` сначала пытается создать per-user Scheduled Task.
- Добавлено описание автоматического fallback в папку Startup текущего пользователя, если Windows отвечает `Access is denied` или `Отказано в доступе`.
- Уточнено, что `status` и `doctor` показывают фактический способ автозапуска: `scheduled_task` или `startup_folder`.
- Уточнено, что диагностика автозапуска показывает реальную цель автозапуска, если она обнаружена.
- Обновлены примеры использования и ограничения беты с учетом fallback-сценария.

### Notes

- Основной приоритет по-прежнему у Scheduled Task, потому что это штатный способ фонового запуска watcher при входе пользователя в Windows.
- Startup folder рассматривается как пользовательский fallback для систем, где Task Scheduler недоступен по правам или политике.

## [v0.1.0-beta.1] - 2026-04-21

Первая публично оформленная бета документации и пользовательского сценария для Windows-версии ProtoSwitch.

### Added

- Описан продукт как Windows-first Rust CLI/TUI для Telegram Desktop.
- Зафиксирован основной набор команд: `init`, `watch`, `status`, `switch`, `doctor`, `autostart install`, `autostart remove`.
- Описаны рабочие пути для конфигурации, runtime state и логов watcher.
- Добавлено объяснение потока смены MTProto proxy через `mtproto.ru` и `tg://proxy`.
- Добавлена Mermaid-схема процесса `check -> fetch -> parse -> apply -> recheck`.
- Зафиксированы ограничения текущей бета-версии: Windows-only, Telegram Desktop-only, один источник proxy, best-effort health-check.
- Добавлена явная диагностика ошибок Task Scheduler при `autostart install/remove`.

### Notes

- Версия ориентирована на пользователей Windows 10/11.
- Прямое редактирование `tdata` не предполагается; поддерживаемый путь применения proxy идет через официальный deep link Telegram.
- Поддержка macOS вынесена в следующий этап развития проекта.
