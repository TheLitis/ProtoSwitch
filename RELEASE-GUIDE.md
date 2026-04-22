# Release Guide

Этот файл нужен для воспроизводимой публикации Windows-релизов ProtoSwitch через GitHub Releases.

## Что должно попасть в релиз `v0.1.0-beta.9`

- `ProtoSwitch-Setup-x64.exe`
- `protoswitch-portable-win-x64.zip`
- актуальные release notes на основе `CHANGELOG.md`

## Перед публикацией

1. Убедитесь, что версия релиза совпадает в документации, верхней записи `CHANGELOG.md`, `Cargo.toml` и git tag.
2. Соберите артефакты:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-distribution.ps1`
3. Прогоните portable smoke:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-portable.ps1`
4. Прогоните installer smoke на чистой Windows-сессии без установленного ProtoSwitch:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-installer.ps1 -Mode CurrentUser`
5. Если нужна проверка `machine-wide`, откройте повышенный PowerShell и запустите:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-installer.ps1 -Mode Both`
6. Проверьте, что README описывает текущий релизный сценарий без расхождений с фактической поставкой.

`smoke-installer.ps1` по умолчанию требует чистую машину: без уже установленного ProtoSwitch, без активного `protoswitch.exe`, без существующего `scheduled_task` или `startup_folder`. Это сделано специально, чтобы smoke не трогал рабочую установку пользователя. Ключ `-AllowDirtyEnvironment` оставлен только для одноразовых экспериментов в disposable-окружении.

## Публикация на GitHub Releases

1. Выполните scripted publish:
   `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\publish-release.ps1`
2. Скрипт сам:
   - берет версию из `Cargo.toml`;
   - ищет `ProtoSwitch-Setup-x64.exe` и `protoswitch-portable-win-x64.zip` в `dist\<version>`;
   - вырезает верхнюю запись из `CHANGELOG.md`;
   - пишет временный notes-файл в `UTF-8 without BOM`;
   - вызывает `gh release create` или `gh release edit` только через `--notes-file`;
   - перезаливает ассеты через `--clobber`, если релиз уже существует;
   - делает self-check опубликованного `body` и падает, если в начале появился `U+FEFF`.
3. Не вставляйте русские release notes вручную через буфер обмена и не передавайте их через `--notes`: именно это ломало кодировку в прошлых релизах.

## Что должно быть в верхней записи CHANGELOG

- Названия обоих артефактов.
- Что обычный запуск теперь открывает более спокойную terminal-first console с горячими клавишами для `switch`, `cleanup`, `doctor`, `settings`, `autostart` и `watcher`.
- Что ProtoSwitch теперь валидирует новые proxy до применения и пишет явный статус источника и текущего proxy.
- Что при применении proxy приложение старается вернуть фокус в предыдущее окно и не оставлять Telegram поверх рабочего стола.
- Что installer теперь предлагает отдельную галочку для desktop shortcut на финальной странице.
- Что `protoswitch.exe` и installer теперь используют иконку с прозрачным фоном.
- Разницу между per-user и machine-wide установкой.
- Напоминание, что machine-wide install не делает ProtoSwitch Windows Service.
- Правило автозапуска: сначала `scheduled_task`, затем `startup_folder`.
- Поведение uninstall: uninstaller сам снимает `scheduled_task` или `startup_folder`, бинарники и installer-следы удаляются, пользовательские `config/state/logs` сохраняются.

## Проверка после публикации

1. Скачайте installer из опубликованного релиза и проверьте имя файла.
2. Скачайте portable-архив и проверьте имя файла.
3. Убедитесь, что release notes не противоречат `README.md` и `CHANGELOG.md`.
4. Ручной UI-check: на финальной странице installer по-прежнему должен быть чекбокс desktop shortcut. Silent smoke это не покрывает, потому что этот сценарий завязан на `postinstall` UI.
5. Проверьте, что пользователь по описанию понимает:
   - какой артефакт выбрать;
   - как обновляться вручную;
   - что происходит с автозапуском;
   - что удаляется при uninstall, а что остается в профиле.
