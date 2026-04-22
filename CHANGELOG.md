# Changelog

Все заметные изменения проекта ProtoSwitch будут отражаться в этом файле.

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
