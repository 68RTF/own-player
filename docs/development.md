# Разработка

Для сборки нужны Rust/Cargo, заголовки GTK 3 и `pkg-config`. Для работы приложения нужны GTK 3, mpv, ffmpeg/ffprobe и curl; `secret-tool` нужен для сохранения токена SoundCloud в связке ключей. Инструкция по установке из исходников находится в [README](../README.md#из-исходников).

## Сборка пакетов

На Arch Linux:

```sh
bash scripts/build-arch.sh
```

На соответствующей системе Ubuntu 24.04 или Debian 13:

```sh
bash scripts/build-deb.sh ubuntu24.04
# или на Debian 13
bash scripts/build-deb.sh debian13
```

Пакеты появляются в `dist/`. Для Arch дополнительно нужны инструменты `makepkg` и зависимости из [PKGBUILD](../packaging/arch/PKGBUILD).

## Проверка

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
scripts/check-ui.sh
cargo build --release --locked --bins
```

UI-тест требует Xvfb, ImageMagick, ffmpeg и mpv. Он запускается с временными данными и не обращается к внешним сервисам. Live-тесты SoundCloud, LRCLIB и Яндекс Музыки по умолчанию пропущены. GitHub Actions проверяет проект и собирает отдельные пакеты для трёх дистрибутивов; при публикации тега `v*` пакеты прикладываются к релизу.

## Данные

После переименования сохранён прежний каталог данных `~/.local/share/own-player`, чтобы не потерять настройки и очередь. Индекс повторных скачиваний SoundCloud находится в `soundcloud-downloads.json` внутри этого каталога и создаётся с правами 0600. Временные файлы отменённой загрузки удаляются. Найденные тексты и обложки сохраняются в кэш.
