# Свой плеер

Локальный музыкальный плеер для Linux на Rust, GTK 3 и mpv. Поддерживает очередь, поиск, перемешивание, повтор, тексты песен, обложки, скорость и громкость. Очередь и настройки сохраняются в каталоге XDG. Онлайн-функции необязательны: поиск текстов через LRCLIB, обложек по подключённому CSV Яндекс Музыки и загрузка оригиналов SoundCloud, если автор разрешил скачивание.

## Установка

Готовые пакеты для Arch Linux, Ubuntu 24.04 и Debian 13 будут доступны в [GitHub Releases](https://github.com/68RTF/own-player/releases) после публикации релиза. Пакеты собираются раздельно на соответствующих системах. Для Arch пакет `.pkg.tar.zst` устанавливается через `sudo pacman -U ./own-player-*.pkg.tar.zst`; для Ubuntu и Debian используй соответствующий `.deb` и `sudo apt install ./own-player-*.deb`.

Для сборки из исходников нужны Rust/Cargo, заголовки GTK 3 и `pkg-config`. Во время работы нужны GTK 3, mpv, ffmpeg/ffprobe и curl. `secret-tool` (libsecret) нужен только для сохранения токена SoundCloud в связке ключей.

```sh
cargo build --release --locked --bins
install -Dm755 target/release/own-player ~/.local/bin/own-player
install -Dm755 target/release/ym-export ~/.local/bin/ym-export
install -Dm644 own-player.desktop ~/.local/share/applications/own-player.desktop
install -Dm644 assets/own-player.svg ~/.local/share/icons/hicolor/scalable/apps/own-player.svg
```

Для создания пакета Arch из текущих исходников: `bash scripts/build-arch.sh`. Для Ubuntu 24.04 или Debian 13: `bash scripts/build-deb.sh ubuntu24.04` или `bash scripts/build-deb.sh debian13` на соответствующей системе. Результат находится в `dist/`.

## Использование

Добавь файлы или папку через боковую панель. Трек в очереди запускается двойным щелчком или Enter; порядок меняется перетаскиванием либо через меню. Поддерживаются локальные `.lrc`/`.txt`, встроенные тексты и автоматический поиск в LRCLIB; найденное кэшируется. Обложки берутся из файла или папки. Поиск обложек Яндекс Музыки работает после подключения CSV и может быть отключён в настройках.

Загрузка SoundCloud ограничена треками с разрешённым скачиванием оригинала. Авторизация хранится через `secret-tool` либо только до закрытия приложения, если связка ключей недоступна. Временные файлы отменённой загрузки удаляются; файл индекса повторных скачиваний хранится в `~/.local/share/own-player/soundcloud-downloads.json` с правами 0600. Пакетная загрузка пишет список пропущенных треков в выбранную папку.

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
