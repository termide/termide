# Руководство по установке

Это руководство описывает различные способы установки TermIDE на вашу систему.

## Скачать готовый бинарный файл (Рекомендуется)

Самый простой способ начать работу — скачать готовый бинарный файл для вашей платформы.

### Шаг 1: Скачивание

Посетите страницу [GitHub Releases](https://github.com/termide/termide/releases) и скачайте последнюю версию для вашей платформы:

**Linux x86_64** (также работает в WSL/WSL2):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.2.0-x86_64-unknown-linux-gnu.tar.gz
```

**Linux ARM64** (Raspberry Pi, ARM серверы):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.2.0-aarch64-unknown-linux-gnu.tar.gz
```

**macOS Intel (x86_64)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.2.0-x86_64-apple-darwin.tar.gz
```

**macOS Apple Silicon (M1/M2/M3)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.2.0-aarch64-apple-darwin.tar.gz
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

## Установка из crates.io

Если у вас установлен Rust, вы можете установить TermIDE напрямую из crates.io:

```bash
cargo install termide
```

Бинарный файл будет установлен в `~/.cargo/bin/termide`.

Убедитесь, что `~/.cargo/bin` добавлен в PATH:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
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
```

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

### Windows (WSL)

TermIDE работает в Windows Subsystem for Linux (WSL и WSL2):

1. Установите WSL2, если еще не установлен
2. Скачайте Linux x86_64 бинарный файл внутри WSL:
   ```bash
   wget https://github.com/termide/termide/releases/latest/download/termide-0.2.0-x86_64-unknown-linux-gnu.tar.gz
   tar xzf termide-0.2.0-x86_64-unknown-linux-gnu.tar.gz
   ./termide
   ```

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
