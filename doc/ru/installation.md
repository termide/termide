# Руководство по установке

Это руководство описывает различные способы установки TermIDE на вашу систему.

## Скачать готовый бинарный файл (Рекомендуется)

Самый простой способ начать работу — скачать готовый бинарный файл для вашей платформы.

### Шаг 1: Скачивание

Посетите страницу [GitHub Releases](https://github.com/termide/termide/releases) и скачайте последнюю версию для вашей платформы:

**Linux x86_64** (также работает в WSL/WSL2):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-x86_64-unknown-linux-gnu.tar.gz
```

**Linux x86_64 — статический musl** (Alpine, distroless-контейнеры,
любая система без glibc):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-x86_64-unknown-linux-musl.tar.gz
```

**Linux ARM64** (Raspberry Pi, ARM серверы):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-aarch64-unknown-linux-gnu.tar.gz
```

**Linux ARM64 — статический musl** (Android/Termux, Alpine ARM, любая ARM64-система без glibc):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-aarch64-unknown-linux-musl.tar.gz
```

**macOS Intel (x86_64)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.29.1-x86_64-apple-darwin.tar.gz
```

**macOS Apple Silicon (M1/M2/M3)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.29.1-aarch64-apple-darwin.tar.gz
```

### Шаг 2: Распаковка

```bash
tar xzf termide-*.tar.gz
```

### Шаг 3: Запуск

```bash
./termide
```

### Шаг 4: Установка в систему (Опционально)

Чтобы установить TermIDE системно, переместите бинарный файл в директорию из PATH:

```bash
# Linux
sudo mv termide /usr/local/bin/

# macOS
sudo mv termide /usr/local/bin/
```

Теперь вы можете запускать `termide` из любого места в терминале.

## Переносимый статический бинарник (Alpine / контейнеры)

С каждым релизом публикуется полностью статическая сборка **musl**: она не
линкует разделяемых библиотек и работает на любом дистрибутиве Linux, включая
Alpine и минимальные контейнеры. Весь проект на чистом Rust (rustls + russh +
russh-sftp — без OpenSSL и libssh2), поэтому это тот же код, просто собранный
под musl.

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.29.1-x86_64-unknown-linux-musl.tar.gz
./termide

# Проверка полной статичности — нет разделяемых библиотек
ldd ./termide   # → "not a dynamic executable"
```

Чтобы собрать самостоятельно (например, под другой вариант musl), flake
предоставляет рецепт как деривацию:

```bash
nix build github:termide/termide#termide-static
./result/bin/termide
```

Бинарник можно скопировать в контейнер или урезанный образ Alpine — он работает
без установленных `musl-dev` или `glibc`. (Сборка ARM64 musl используется и для
[Android / Termux](#android-termux).)

## Установка через пакетный менеджер

### Debian/Ubuntu (.deb)

```bash
wget https://github.com/termide/termide/releases/latest/download/termide_0.29.1-1_amd64.deb
sudo dpkg -i termide_0.29.1-1_amd64.deb
```

### Fedora/RHEL/CentOS (.rpm)

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-1.x86_64.rpm
sudo rpm -i termide-0.29.1-1.x86_64.rpm
```

### Arch Linux (AUR)

```bash
# Сборка из исходников
yay -S termide

# Или установка готового бинарника
yay -S termide-bin
```

### Homebrew (macOS/Linux)

```bash
brew tap termide/termide
brew install termide
```

### NixOS/Nix (Flakes)

```bash
# Запуск без установки
nix run github:termide/termide

# Установка в профиль пользователя
nix profile install github:termide/termide
```

## Сборка из исходников

### Требования

- **Rust 1.70+** (stable)
- **Git**

### Используя Cargo

```bash
# Клонирование репозитория
git clone https://github.com/termide/termide.git
cd termide

# Сборка в release режиме
cargo build --release

# Бинарный файл будет в target/release/termide
./target/release/termide

# Опционально: Установка в ~/.cargo/bin
cargo install --path .

# Или установка напрямую из репозитория, без клонирования:
cargo install --git https://github.com/termide/termide --locked
```

> **Примечание:** TermIDE **не** публикуется на crates.io. `cargo install termide`
> скачает устаревший посторонний ранний релиз — всегда собирайте из исходников
> (клонирование выше) или используйте `cargo install --git …`.

### Используя Nix (с Flakes)

```bash
# Клонирование репозитория
git clone https://github.com/termide/termide.git
cd termide

# Вход в окружение разработки
nix develop

# Сборка с cargo
cargo build --release

# Или сборка напрямую с Nix
nix build
```

## Особенности для различных платформ

### Linux

Для готового бинарного файла не требуется дополнительных зависимостей.

При сборке из исходников могут потребоваться пакеты разработки:
```bash
# Debian/Ubuntu
sudo apt-get install build-essential

# Fedora/RHEL
sudo dnf install gcc
```

### macOS

При первом запуске macOS может заблокировать приложение, так как оно не подписано. Чтобы разрешить запуск:
1. Кликните правой кнопкой на `termide` и выберите "Открыть"
2. Нажмите "Открыть" в диалоге безопасности

Альтернативно, удалите атрибут карантина:
```bash
xattr -d com.apple.quarantine termide
```

### Windows (нативный)

TermIDE работает нативно на Windows 10+ через ConPTY. Для лучшего опыта рекомендуется Windows Terminal.

1. Скачайте `.zip` архив со страницы [GitHub Releases](https://github.com/termide/termide/releases):
   - `termide-VERSION-x86_64-pc-windows-msvc.zip`
2. Распакуйте архив
3. Запустите `termide.exe` в Windows Terminal

**Пути конфигурации:**
- Конфиг: `%APPDATA%\termide\config.toml`
- Сессии: `%APPDATA%\termide\sessions\`
- Логи: `%LOCALAPPDATA%\termide\cache\termide.log`

### Windows (WSL)

TermIDE также работает в Windows Subsystem for Linux (WSL и WSL2):

1. Установите WSL2, если еще не установлен
2. Скачайте Linux x86_64 бинарный файл внутри WSL:
   ```bash
   wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-x86_64-unknown-linux-gnu.tar.gz
   tar xzf termide-0.29.1-x86_64-unknown-linux-gnu.tar.gz
   ./termide
   ```

### Android (Termux)

TermIDE работает в [Termux](https://termux.dev). Используйте **статическую
сборку ARM64 musl** — glibc-сборка `aarch64-unknown-linux-gnu` не запустится на
Android (libc Bionic):

```bash
pkg install git openssh   # инструменты, к которым обращается termide (плюс нужные LSP)
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.1-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.29.1-aarch64-unknown-linux-musl.tar.gz
./termide
```

Системный буфер обмена на Android недоступен (нет X11/Wayland), а монитор
ресурсов может показывать неполные данные (ограниченный `/proc`); редактор,
файловый менеджер, git и встроенный терминал работают как обычно.

## Проверка установки

После установки проверьте, что всё работает:

```bash
termide --version
```

## Следующие шаги

- Прочитайте [Руководство по интерфейсу](ui.md) чтобы понять структуру приложения
- Изучите горячие клавиши [Файлового менеджера](file-manager.md)
- Исследуйте возможности [Терминала](terminal.md) и [Редактора](editor.md)
- Настройте внешний вид с помощью [Тем](themes.md)
