# Veyra

![Плеер Veyra](docs/player.png)

Музыкальный плеер для Linux на Rust, GTK 3 и mpv.

## Возможности

- Локальная библиотека и очередь с поиском, перемешиванием и повтором.
- Тексты песен и обложки, автоматический поиск недостающих текстов через LRCLIB.
- Регулировка скорости и громкости.
- Поиск обложек по CSV Яндекс Музыки — его можно отключить в настройках.
- Ручная загрузка оригиналов SoundCloud, если автор разрешил скачивание.

## Установка

Пакеты для Arch Linux, Ubuntu 24.04 и Debian 13 лежат в [релизах](https://github.com/68RTF/veyra/releases). Для Arch Linux (x86_64) скачай пакет `.pkg.tar.zst` и установи его командой `sudo pacman -U ./veyra-0.7.0-1-x86_64.pkg.tar.zst`. Для Ubuntu 24.04 (amd64) скачай `veyra_0.7.0_ubuntu24.04_amd64.deb`, для Debian 13 (amd64) — `veyra_0.7.0_debian13_amd64.deb`. Установи скачанный пакет командой `sudo apt install ./ИМЯ_СКАЧАННОГО_ФАЙЛА.deb`.

Для сборки из исходников нужны Rust/Cargo, заголовки GTK 3 и `pkg-config`. Во время работы нужны GTK 3, mpv, ffmpeg/ffprobe и curl. `secret-tool` (libsecret) нужен только для сохранения токена SoundCloud в связке ключей.

```sh
cargo build --release --locked --bins
install -Dm755 target/release/veyra ~/.local/bin/veyra
install -Dm755 target/release/ym-export ~/.local/bin/ym-export
install -Dm644 veyra.desktop ~/.local/share/applications/veyra.desktop
install -Dm644 assets/veyra.svg ~/.local/share/icons/hicolor/scalable/apps/veyra.svg
```

Для создания пакета Arch из текущих исходников: `bash scripts/build-arch.sh`. Для Ubuntu 24.04 или Debian 13: `bash scripts/build-deb.sh ubuntu24.04` или `bash scripts/build-deb.sh debian13` на соответствующей системе. Результат находится в `dist/`.

## Использование

Добавь файлы или папку через боковую панель. Трек в очереди запускается двойным щелчком или Enter; порядок меняется перетаскиванием либо через меню. Поддерживаются локальные `.lrc`/`.txt`, встроенные тексты и автоматический поиск в LRCLIB; найденное кэшируется. Обложки берутся из файла или папки. Поиск обложек Яндекс Музыки работает после подключения CSV и может быть отключён в настройках.

Загрузка SoundCloud ограничена треками с разрешённым скачиванием оригинала. Авторизация хранится через `secret-tool` либо только до закрытия приложения, если связка ключей недоступна. Временные файлы отменённой загрузки удаляются; файл индекса повторных скачиваний хранится в `~/.local/share/own-player/soundcloud-downloads.json` с правами 0600. Старый каталог данных сохранён, чтобы настройки и очередь остались на месте после переименования. Пакетная загрузка пишет список пропущенных треков в выбранную папку.

Экспорт метаданных плейлиста Яндекс Музыки без аудио:

```sh
ym-export 'PLAYLIST_URL' -o playlist.csv
```

## Проверка

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
scripts/check-ui.sh
cargo build --release --locked --bins
```

UI-тест требует Xvfb, ImageMagick, ffmpeg и mpv. Он запускается изолированно с временными данными и не вызывает внешние сервисы. Live-тесты SoundCloud, LRCLIB и Яндекс Музыки по умолчанию пропущены. GitHub Actions собирает отдельные пакеты для трёх дистрибутивов и прикладывает их к релизу для тега `v*`.

Исходный проект пока не содержит лицензии на повторное использование кода.
