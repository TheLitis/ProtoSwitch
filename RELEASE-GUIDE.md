# Release Guide

Этот файл нужен только для выпуска релизов ProtoSwitch.

## Что Должно Попасть В `v0.2.0-beta.4`

- `ProtoSwitch-Setup-x64.exe`
- `protoswitch-portable-win-x64.zip`
- `protoswitch-portable-linux-x64.tar.gz`
- `protoswitch-portable-linux-arm64.tar.gz`
- `protoswitch-portable-macos-x64.tar.gz`
- `protoswitch-portable-macos-arm64.tar.gz`

## Локальная Подготовка

1. Проверьте, что версия совпадает в `Cargo.toml`, верхней записи `CHANGELOG.md` и git tag.
2. Прогоните общий набор:
   `cargo test --locked`
3. Прогоните детерминированный watcher e2e отдельно:
   `cargo test --locked watcher_e2e_`
4. Соберите Windows-артефакты локально:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-distribution.ps1`
5. Прогоните portable smoke:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-portable.ps1 -Version 0.2.0-beta.4`
6. Прогоните installer smoke на чистой Windows-сессии:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-installer.ps1 -Mode CurrentUser`
7. Если нужна machine-wide проверка, используйте повышенный PowerShell:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-installer.ps1 -Mode Both`
8. Для ручной live-проверки на реальном Telegram Desktop используйте opt-in сценарий:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\e2e-windows-live.ps1 -ConfirmLiveMutation`
9. Linux/macOS portable smoke идёт через CI-скрипт:
   `python3 scripts/smoke-unix.py --repo-root <repo> --version 0.2.0-beta.4 --platform linux --arch x64`

## Основной Путь Публикации

1. Закоммитьте релизные изменения.
2. Создайте подписанный тег `v0.2.0-beta.4`.
3. Запушьте `main` и тег.
4. GitHub Actions workflow `.github/workflows/release.yml` сам:
   - проверит наличие записи в `CHANGELOG.md`;
   - прогонит Windows deterministic watcher e2e;
   - соберёт Windows x64, Linux x64/arm64 и macOS x64/arm64;
   - прогонит Windows/Linux/macOS smoke там, где это возможно автоматически;
   - упакует все portable-артефакты;
   - создаст или обновит GitHub Release.

## Ручной Recovery Path

Если нужно вручную поправить notes или перезалить ассеты в уже существующий релиз:

`powershell -NoProfile -ExecutionPolicy Bypass -File scripts\publish-release.ps1`

Скрипт:

- берёт версию из `Cargo.toml`;
- вырезает верхнюю запись из `CHANGELOG.md`;
- пишет notes как `UTF-8 without BOM`;
- работает через `gh --notes-file`, чтобы не ломать русскую кодировку;
- перезаливает все файлы, которые лежат в `dist\<version>`.

Этим способом стоит пользоваться только тогда, когда `dist\<version>` уже содержит полный набор артефактов.

## Что Проверить После Публикации

1. В релизе лежат все 6 файлов.
2. README и release notes читаются без mojibake.
3. Installer называется `ProtoSwitch-Setup-x64.exe`.
4. Linux/macOS portable-архивы названы по схеме `protoswitch-portable-<platform>-<arch>.tar.gz`.
5. Верхняя запись `CHANGELOG.md` совпадает с опубликованными notes.
