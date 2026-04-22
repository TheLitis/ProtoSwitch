# Release Guide

Этот файл нужен для ручной публикации Windows-релизов ProtoSwitch через GitHub Releases.

## Что должно попасть в релиз `v0.1.0-beta.4`

- `ProtoSwitch-Setup-x64.exe`
- `protoswitch-portable-win-x64.zip`
- актуальные release notes на основе `CHANGELOG.md`

## Перед публикацией

1. Убедитесь, что версия релиза совпадает в документации, release notes и git tag.
2. Проверьте, что installer и portable-архив относятся к одному и тому же релизному билду.
3. Убедитесь, что в portable-архиве лежат пользовательские файлы поставки, а installer ставит `protoswitch.exe`, ярлык запуска и uninstall entry.
4. Проверьте, что README описывает текущий релизный сценарий без расхождений с фактической поставкой.

## Публикация на GitHub Releases

1. Создайте или откройте релиз с тегом нужной версии.
2. Загрузите `ProtoSwitch-Setup-x64.exe`.
3. Загрузите `protoswitch-portable-win-x64.zip`.
4. Вставьте release notes на основе верхней записи в `CHANGELOG.md`.
5. В тексте релиза явно укажите:
   - что доступны installer и portable-вариант;
   - что installer поддерживает `только для текущего пользователя` и `для всех пользователей`;
   - что чекбокс автозапуска watcher включен по умолчанию;
   - что если Scheduled Task не удалось создать, ProtoSwitch переходит на `startup_folder`;
   - что обновление выполняется вручную через GitHub Releases.
6. Опубликуйте релиз.

## Что указать в release notes

- Названия обоих артефактов.
- Разницу между per-user и machine-wide установкой.
- Напоминание, что machine-wide install не делает ProtoSwitch Windows Service.
- Правило автозапуска: сначала `scheduled_task`, затем `startup_folder`.
- Поведение uninstall: uninstaller сам снимает `scheduled_task` или `startup_folder`, бинарники и installer-следы удаляются, пользовательские `config/state/logs` сохраняются.

## Проверка после публикации

1. Скачайте installer из опубликованного релиза и проверьте имя файла.
2. Скачайте portable-архив и проверьте имя файла.
3. Убедитесь, что release notes не противоречат `README.md` и `CHANGELOG.md`.
4. Проверьте, что пользователь по описанию понимает:
   - какой артефакт выбрать;
   - как обновляться вручную;
   - что происходит с автозапуском;
   - что удаляется при uninstall, а что остается в профиле.
