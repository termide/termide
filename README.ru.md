# TermIDE

[![GitHub Release](https://img.shields.io/github/v/release/termide/termide)](https://github.com/termide/termide/releases)
[![CI](https://github.com/termide/termide/actions/workflows/release.yml/badge.svg)](https://github.com/termide/termide/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | [中文](README.zh.md) | **Русский**

Терминальное рабочее место «всё в одном» для рабочей машины и серверов: редактор кода с LSP, двухпанельный файловый менеджер с SFTP/FTP, терминал, git, просмотр баз данных и агент для кода — в одном статическом бинарнике на Rust без настройки.

**[Сайт](https://termide.github.io)** | **[Документация](doc/ru/README.md)** | **[Релизы](https://github.com/termide/termide/releases)** | **[Скриншоты](https://termide.github.io/ru/#screenshots)**

<p align="center"><img src="assets/screenshots/termide.gif" alt="TermIDE — редактор, файловый менеджер, терминал и просмотрщики в одном TUI" width="900"></p>

## Почему TermIDE?

Терминальные редакторы закрывают работу с кодом, а всё вокруг — файлы на удалённых хостах, базы данных, git, долгоживущие шеллы, агент для кода — обычно требует плагинов или отдельных утилит. TermIDE поставляет всё это в одном бинарнике, который работает из коробки на ноутбуке, сервере или телефоне:

| Возможность | TermIDE | Fresh | Vim/Neovim | Helix | Micro |
|---------|:-------:|:-----:|:----------:|:-----:|:-----:|
| Поддержка LSP | ✓ | ✓ | ✓ | ✓ | плагин |
| Нулевая настройка | ✓ | ✓ | ✗ | ✓ | ✓ |
| Автоматизация скриптами | ✓ | ✓ | ✓ | ✗ | плагин |
| Внешние агенты (Claude Code, Codex, Gemini CLI) | ✓ | ✓ | плагин | ✗ | ✗ |
| Удалённые ФС (SFTP/FTP) | ✓ | SSH | ✓ | ✗ | ✗ |
| Просмотр Markdown | ✓ | ✓ | плагин | ✗ | ✗ |
| Встроенный терминал | ✓ | ✓ | плагин | ✗ | ✗ |
| Интеграция с Git | ✓ | ✓ | плагин | ✗ | ✗ |
| Раскладки проектов | ✓ | ✓ | плагин | ✗ | ✗ |
| Многопанельный интерфейс | ✓ | ✓ | плагин | ✗ | ✗ |
| Закладки | ✓ | ✓ | плагин | ✗ | ✗ |
| Hex / бинарный просмотрщик | ✓ | ✗ | плагин | ✗ | плагин |
| Просмотр архивов (zip/tar) | ✓ | ✗ | ✓ | ✗ | ✗ |
| Файловый менеджер | ✓ | только дерево | плагин | ✗ | ✗ |
| Отсоединяемые экземпляры | ✓ | ✓ | ✗ | ✗ | ✗ |
| Встроенный агент для кода (локальные и облачные модели) | ✓ | ✗ | плагин | ✗ | ✗ |
| MCP-серверы | ✓ | ✗ | плагин | ✗ | ✗ |
| Просмотр баз данных | ✓ | ✗ | плагин | ✗ | ✗ |
| Просмотр диаграмм (Mermaid) | ✓ | ✗ | плагин | ✗ | ✗ |
| Просмотр изображений | ✓ | ✗ | плагин | ✗ | ✗ |
| Фоновые файловые операции | ✓ | ✗ | плагин | ✗ | ✗ |
| Монитор ресурсов | ✓ | ✗ | ✗ | ✗ | ✗ |

**TermIDE = Редактор + Файловый менеджер + Терминал + Git + Агент в одном TUI-приложении.**

## Возможности

- **Терминальная IDE** - Подсветка синтаксиса для 23 языков, навигация по словам (Ctrl+Left/Right), навигация по абзацам/символам (Ctrl+Up/Down), переключение комментариев (Ctrl+/), автоотступы, автозакрытие скобок
- **Поддержка LSP** - Автодополнение, поиск ссылок (Shift+F12), переименование символа (F4), переход к определению (Ctrl+Click), диагностика
- **Агент для кода** - Панель (`Alt+A`), в которой языковая модель читает, правит и запускает команды в вашем проекте через любой OpenAI- или Anthropic-совместимый endpoint (локальные llama.cpp / Ollama / vLLM / omlx или облачные), спрашивая разрешение на каждый вызов инструмента, с `/undo` и контрольными точками, чтобы откатить его правки; скиллы, шаблоны промптов, MCP-серверы, хуки команд и внешние агенты по ACP (Claude Code, Codex, Gemini CLI) в той же панели
- **Умный файловый менеджер** - Древовидный вид с разворачиваемыми каталогами, вложенный git-статус, пакетные операции, поиск по файлам/содержимому (glob/regex), инкрементальный поиск в дереве; архивы zip, tar и ISO открываются как каталоги только для чтения (в том числе на сервере и внутри другого архива), а `P` упаковывает выделенное в zip или tar
- **Удалённые файловые системы** - Просмотр и редактирование файлов на удалённых серверах прямо из файлового менеджера по SFTP / FTP / FTPS, копирование между локальной и удалённой панелями — на чистом Rust (russh + rustls), без нативных библиотек, работает на статическом musl (`smb://` / `nfs://` — через системное монтирование)
- **Фоновые файловые операции** - Копирование, перемещение, загрузка, скачивание, удаление и пакетные передачи выполняются в фоне с прогресс-баром, счётчиком байт/времени и паузой / возобновлением / отменой (панель операций)
- **Встроенный терминал** - Полная поддержка PTY, escape-последовательности VT100, отслеживание мыши
- **Интеграция с Git** - Панель статуса, журнал коммитов с цветным Unicode-графом (откат к ASCII), индексация/деиндексация, ветки с их рабочими копиями (worktree), переключение веток, управление stash, инлайн blame
- **Просмотр баз данных** - Браузер только для чтения для SQLite / PostgreSQL / MySQL, открываемый по URL-закладке: таблица с 2D-курсором по ячейкам, серверная сортировка по столбцу и типозависимая фильтрация по столбцам, постраничная подгрузка скользящим окном и диалог детали строки с копированием в TSV / JSON / INSERT
- **Многопанельный интерфейс** - Вертикально разделённые группы панелей с настраиваемой высотой каждой и переключением полноэкранного режима одной клавишей (`Alt+F11`); умное авто-стекирование при сужении терминала; новые панели открываются после активной
- **Просмотр изображений** - Нативная графика в терминалах Kitty, WezTerm, iTerm2, Ghostty, foot
- **Hex / бинарный просмотрщик и редактор** - Hex/ASCII-вид (адаптивные секции по 16 байт) для бинарных файлов, курсор байта показан в обеих зонах, выделение перетаскиванием/Shift и копирование в буфер, поиск по ASCII и hex-байтам, переключение hex↔текст (`Ctrl+L`); `F4` открывает для перезаписи с резервной копией `.bak` при сохранении
- **Просмотр Markdown** - Рендеренный просмотр (только чтение) для `.md` / `.markdown` (заголовки, списки, таблицы, подсвеченные блоки кода, кликабельные ссылки и пиктограммы изображений) с курсором, выделением и копированием; `Ctrl+E` переключает на редактируемый исходник; встроенные блоки ```mermaid``` рендерятся как диаграммы
- **Просмотр диаграмм Mermaid** - Рендер `.mmd` / `.mermaid` в текстовую псевдографику — flowchart, sequence, state, class, ER, gantt, pie, journey, mindmap, timeline, gitGraph, quadrant; 2D-прокрутка, копирование в буфер и `Ctrl+E` для редактирования исходника
- **Внешние приложения** - Открытие файлов системными приложениями по умолчанию (Shift+Enter)
- **38 встроенных тем** - Тёмные, светлые, ретро и кинематографичные темы (Dracula, Nord, Monokai, Solarized, Matrix, Pip-Boy, Norton Commander, Windows 95 и др.)
- **Пользовательские темы** - Создавайте свои темы в формате TOML
- **15 языков интерфейса** - Бенгальский, китайский, английский, французский, немецкий, хинди, индонезийский, японский, корейский, португальский, русский, испанский, тайский, турецкий, вьетнамский (отсутствующие ключи прозрачно откатываются к английскому)
- **Управление проектами** - Автосохранение и восстановление раскладок панелей для каждого проекта; проекты, с которых вы переключились, остаются открытыми в фоне (терминалы продолжают работать, несохранённые правки сохраняются), а меню «Проекты» и переключатель `Alt+\` показывают открытые и недавние проекты
- **Отсоединяемые экземпляры** - `termide --detached` оставляет экземпляр целиком — редакторы, оболочки, LSP-серверы, запущенные задачи — работать после закрытия терминала; `termide --attach` подхватывает его из любого терминала любого размера (только Unix)
- **Системный монитор** - CPU, RAM, сетевой I/O в реальном времени в меню-баре и использование диска в статус-баре; клик по индикатору открывает модал с деталями (топ процессов по CPU/RAM, топ по сетевым соединениям с прослушиваемыми портами); повторный клик закрывает модал
- **Поиск и замена** - Живой предпросмотр, счётчик совпадений, поддержка regex
- **Пользовательские команды** - Команды оболочки из `commands.toml`, глобальные и проектные, в меню «Команды»: горячие клавиши, группы, формы параметров и режимы терминал / фон / отчёт
- **Модал настроек** - Полноэкранная конфигурация (`Alt+P`) с боковой раскладкой, сгруппированными полями (Внешний вид / Ввод / Раскладка / Производительность / …) и захватом клавиш на месте для всех 9 областей привязок
- **Кроссплатформенность** - Linux (x86_64, ARM64), macOS (Intel, Apple Silicon), Windows (нативно через ConPTY, WSL)
- **Полная поддержка мыши** - Навигация кликом, прокрутка, действия двойным кликом
- **Раскладки клавиатуры** - Поддержка кириллицы с автоматическим переводом горячих клавиш
- **Vim-режим** - Опциональное редактирование в стиле Vim с поддержкой кириллической раскладки
- **Переключатель каталогов** - Быстрая смена каталога по `Ctrl+\`
- **Закладки** - Сохранение и организация часто используемых мест
- **Палитра команд** - Быстрый доступ ко всем командам с нечётким поиском (Ctrl+P)
- **Строка «Открыть»** - Открытие файла, каталога или URL с подсказками путей (Ctrl+G)

## Установка

**Быстрый старт:** Скачайте готовые бинарники с [GitHub Releases](https://github.com/termide/termide/releases) или установите через ваш пакетный менеджер.

**Поддерживаемые платформы:** Linux (x86_64, ARM64), macOS (Intel, Apple Silicon), Windows (x86_64)

### Выберите способ установки

<details open>
<summary><b>📦 Готовые бинарники (рекомендуется)</b></summary>

Скачайте последний релиз для вашей платформы с [GitHub Releases](https://github.com/termide/termide/releases):

```bash
# Linux x86_64 (также работает в WSL)
wget https://github.com/termide/termide/releases/latest/download/termide-0.39.0-x86_64-unknown-linux-gnu.tar.gz
tar xzf termide-0.39.0-x86_64-unknown-linux-gnu.tar.gz
./termide

# Linux x86_64 (статический musl — Alpine, distroless-контейнеры, любая система без glibc)
wget https://github.com/termide/termide/releases/latest/download/termide-0.39.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.39.0-x86_64-unknown-linux-musl.tar.gz
./termide

# macOS Intel (x86_64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.39.0-x86_64-apple-darwin.tar.gz
tar xzf termide-0.39.0-x86_64-apple-darwin.tar.gz
./termide

# macOS Apple Silicon (ARM64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.39.0-aarch64-apple-darwin.tar.gz
tar xzf termide-0.39.0-aarch64-apple-darwin.tar.gz
./termide

# Linux ARM64 (Raspberry Pi, ARM-серверы)
wget https://github.com/termide/termide/releases/latest/download/termide-0.39.0-aarch64-unknown-linux-gnu.tar.gz
tar xzf termide-0.39.0-aarch64-unknown-linux-gnu.tar.gz
./termide

# Linux ARM64 (статический musl — Android/Termux, Alpine ARM, любой ARM64 без glibc)
wget https://github.com/termide/termide/releases/latest/download/termide-0.39.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.39.0-aarch64-unknown-linux-musl.tar.gz
./termide

# Windows x86_64 (скачайте .zip с Releases, распакуйте, запустите в Windows Terminal)
# https://github.com/termide/termide/releases/latest/download/termide-0.39.0-x86_64-pc-windows-msvc.zip
```

</details>

<details>
<summary><b>🪟 Windows (.zip)</b></summary>

TermIDE работает нативно на Windows 10+ через ConPTY. Для лучшего опыта используйте **Windows Terminal**.

1. Скачайте `termide-0.39.0-x86_64-pc-windows-msvc.zip` с [GitHub Releases](https://github.com/termide/termide/releases).
2. Распакуйте архив.
3. Запустите `termide.exe` в Windows Terminal.

Конфигурация, раскладки проектов и логи хранятся в `%APPDATA%\termide\`.

Либо в **WSL/WSL2** используйте сборку Linux x86_64 (`termide-0.39.0-x86_64-unknown-linux-gnu.tar.gz`), как на любом Linux.

</details>

<details>
<summary><b>🐧 Debian/Ubuntu (.deb)</b></summary>

Скачайте и установите пакет `.deb` с [GitHub Releases](https://github.com/termide/termide/releases):

```bash
# Только x86_64 (для ARM64 используйте tar.gz выше)
wget https://github.com/termide/termide/releases/latest/download/termide_0.39.0-1_amd64.deb
sudo dpkg -i termide_0.39.0-1_amd64.deb
```

</details>

<details>
<summary><b>🎩 Fedora/RHEL/CentOS (.rpm)</b></summary>

Скачайте и установите пакет `.rpm` с [GitHub Releases](https://github.com/termide/termide/releases):

```bash
# Только x86_64 (для ARM64 используйте tar.gz выше)
wget https://github.com/termide/termide/releases/latest/download/termide-0.39.0-1.x86_64.rpm
sudo rpm -i termide-0.39.0-1.x86_64.rpm
```

</details>

<details>
<summary><b>🐧 Arch Linux (AUR)</b></summary>

Установите из AUR любимым AUR-помощником:

```bash
# Сборка из исходников
yay -S termide

# Или готовый бинарник
yay -S termide-bin
```

Либо вручную:

```bash
git clone https://aur.archlinux.org/termide.git
cd termide
makepkg -si
```

</details>

<details>
<summary><b>🍺 Homebrew (macOS/Linux)</b></summary>

Установите через Homebrew tap:

```bash
brew tap termide/termide
brew install termide
```

</details>

<details>
<summary><b>❄️ NixOS/Nix (Flakes)</b></summary>

Установите через Nix flakes:

```bash
# Запуск без установки
nix run github:termide/termide

# Установка в профиль пользователя
nix profile install github:termide/termide

# Или добавьте в NixOS configuration.nix
{
  nixpkgs.overlays = [
    (import (builtins.fetchTarball "https://github.com/termide/termide/archive/main.tar.gz")).overlays.default
  ];
  environment.systemPackages = [ pkgs.termide ];
}
```

</details>

<details>
<summary><b>🤖 Android (Termux)</b></summary>

В [Termux](https://termux.dev) используйте сборку **статического ARM64 musl** (сборка glibc
`aarch64-unknown-linux-gnu` не работает на Bionic libc Android):

```bash
pkg install git openssh   # инструменты, которые вызывает termide (а также нужные LSP-серверы)
wget https://github.com/termide/termide/releases/latest/download/termide-0.39.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.39.0-aarch64-unknown-linux-musl.tar.gz
./termide
```

Примечания: на Android нет системного буфера обмена (нет X11/Wayland), а монитор ресурсов
может показывать неполные данные из-за ограниченного `/proc`. Редактор, файловый менеджер,
git и встроенный терминал работают штатно.

</details>

<details>
<summary><b>🔨 Сборка из исходников (Cargo)</b></summary>

Сборка из исходников с помощью Cargo:

```bash
# Клонировать репозиторий
git clone https://github.com/termide/termide.git
cd termide

# Собрать и запустить
cargo run --release
```

</details>

<details>
<summary><b>🔨 Сборка из исходников (Nix)</b></summary>

Сборка из исходников с помощью Nix (для разработки):

```bash
# Клонировать репозиторий
git clone https://github.com/termide/termide.git
cd termide

# Войти в окружение разработки (включает Rust-тулчейн и все зависимости)
nix develop

# Собрать проект
cargo build --release

# Запустить
./target/release/termide
```

</details>

<details>
<summary><b>📦 Переносимый статический бинарник (Alpine / любой Linux)</b></summary>

С каждым релизом публикуется полностью статическая сборка musl. Она не линкует
разделяемых библиотек и работает на любом дистрибутиве Linux, включая Alpine и
минимальные контейнеры. Весь проект на чистом Rust (rustls + russh + russh-sftp —
без OpenSSL и libssh2), поэтому это тот же код, просто собранный под musl.

Проще всего взять готовый tarball из релиза:

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.39.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.39.0-x86_64-unknown-linux-musl.tar.gz
./termide

# Проверка полной статичности — нет разделяемых библиотек
ldd ./termide   # → "not a dynamic executable"
```

Если хотите собрать сами (например, под другой вариант musl), flake предоставляет
тот же рецепт как деривацию:

```bash
nix build github:termide/termide#termide-static
./result/bin/termide
```

Любой из бинарников можно скопировать куда угодно — в контейнер, урезанный образ
Alpine, embedded-устройство — и он будет работать без установленных musl-dev или glibc.

</details>

## Требования

- Для готовых бинарников: дополнительных требований нет
- Для сборки из исходников:
  - Rust 1.70+ (stable)
  - Для пользователей Nix: Nix с включёнными flakes

### Опции командной строки

```
termide [OPTIONS] [FILE]...

Аргументы:
  [FILE]...            Файл(ы) или каталоги для открытия. С путём TermIDE
                       стартует в чистом виде (раскладка проекта не
                       восстанавливается и не сохраняется). Текст открывается
                       в редакторе, поэтому TermIDE работает как $EDITOR для
                       git, crontab, visudo и т. д.; изображения, файлы SQLite,
                       прочие бинарные файлы и каталоги — в своём просмотрщике,
                       hex-редакторе или файловом менеджере.

Опции:
  --log-level <LEVEL>  Уровень логирования (trace, debug, info, warn, error)
  --no-lsp             Отключить LSP-серверы
  -r, --restore        Открыть проекты прошлого запуска в том проекте, где он был
  --config <PATH>      Использовать свой путь к файлу конфигурации
  --diagnostics        Прогнать предполётную диагностику и выйти (без UI)
  --detached           Запустить отсоединённый экземпляр, переживающий закрытие
                       терминала, и напечатать его идентификатор (только Unix)
  --attach [<ID>]      Подключиться к отсоединённому экземпляру, по умолчанию
                       к последнему
  -f, --force          С --attach: перехватить экземпляр у уже подключённого
                       клиента, отключив его
  --kill <ID>          Завершить отсоединённый экземпляр со всеми оболочками и
                       задачами в нём и выйти; несохранённые изменения теряются
  --list-instances     Показать отсоединённые экземпляры и выйти
  --completions <SHELL>
                       Напечатать скрипт автодополнения (bash, zsh, fish) и выйти
  --install-completions [<SHELL>]
                       Установить автодополнение для $SHELL или названной оболочки
  --prompt <PROMPT>    Выполнить одну задачу агента без UI, напечатать ответ
                       в stdout и выйти; `-` читает запрос из stdin
  --agent <NAME>       С --prompt: какое определение агента использовать
  --output <FORMAT>    С --prompt: text (по умолчанию), json или stream-json
  -h, --help           Показать справку
  -V, --version        Показать версию
```

Использование в качестве редактора:

```sh
export EDITOR=termide   # git commit, crontab -e, visudo, ...
```

## Использование

### Быстрый старт

После запуска TermIDE вы увидите раскладку, адаптивную по ширине:
- **Широкие терминалы (>= 160 колонок):** Боковая панель (Git Status в стопке с Operations) + две панели файлового менеджера
- **Обычные терминалы (< 160 колонок):** Боковая панель (Git Status, файловый менеджер и Operations в стопке) + панель файлового менеджера
- Меню-бар сверху, статус-бар снизу

Панели в стопке делят колонку с настраиваемой высотой каждой. `Alt+F11` переключает пресет «полноэкранная текущая панель» (одна панель занимает всю высоту колонки, остальные сворачиваются в строку заголовка); `Ctrl+Alt+=` / `Ctrl+Alt+-` увеличивают / уменьшают фокусную панель на 3 строки.

Используйте `Alt+←/→` для переключения групп панелей, `Alt+↑/↓` для навигации внутри группы, `Alt+M` для открытия меню.

### Документация

Подробная документация:
- **Английский**: [doc/en/README.md](doc/en/README.md)
- **Русский**: [doc/ru/README.md](doc/ru/README.md)
- **Китайский**: [doc/zh/README.md](doc/zh/README.md)

### Горячие клавиши

Все клавиши настраиваются в `config.toml` (см. [Конфигурация](#конфигурация)). Основное:

- **Навигация:** `Alt+M` меню · `Alt+H` помощь · `Alt+Q` выход · `Ctrl+P` палитра команд
- **Панели:** `Alt+←/→` и `Alt+↑/↓` перемещение между/внутри групп · `Alt+K` меню действий панели
- **Проекты:** `Alt+1-9` переход к открытому проекту · `Alt+\` переключатель проектов · `Alt+N` новый проект
- **Открыть:** `Alt+F` Файлы · `Alt+T` Терминал · `Alt+E` Редактор · `Alt+G` Git · `Alt+P` Настройки
- **Файлы и просмотрщики:** `F3` предпросмотр (markdown / диаграмма / hex / изображение) · `Ctrl+E` переключение предпросмотр ↔ исходник · `Ctrl+F` поиск · `Ctrl+R` перечитать с диска · `Ctrl+S` сохранить

📖 Полный справочник по панелям (файловый менеджер, редактор, git, просмотрщики): **[doc/ru/keybindings.md](doc/ru/keybindings.md)**.

## Конфигурация

TermIDE следует [спецификации XDG Base Directory](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html) для организации файлов.

**Расположение файла конфигурации:**
- Linux/BSD: `~/.config/termide/config.toml` (или `$XDG_CONFIG_HOME/termide/config.toml`)
- macOS: `~/Library/Application Support/termide/config.toml`
- Windows: `%APPDATA%\termide\config.toml`

Проект может переопределить любую настройку в `<проект>/.termide/config.toml`.
Настройка с неверным типом или значением игнорируется по отдельности, и об этом
пишется в Журнал; остальной файл продолжает действовать. Следующее сохранение из
настроек перезапишет файл уже без проигнорированной настройки, поэтому перед этим
исходный файл копируется рядом в `config.toml.bak`. `termide --diagnostics` показывает
те же проблемы.

**Расположение данных проектов:**
- Linux/BSD: `~/.local/share/termide/projects/` (или `$XDG_DATA_HOME/termide/projects/`)
- macOS: `~/Library/Application Support/termide/projects/`
- Windows: `%APPDATA%\termide\projects\`

**Расположение файла логов:** каждый запуск пишет свой `session-<date>-<time>.log`
в каталог проекта внутри расположения данных проектов, указанного выше; логи
старше 24 часов удаляются. `logging.file_path` заменяет это одним фиксированным
файлом.

**Расположение закладок:**
- Linux/BSD: `~/.config/termide/bookmarks.toml` (или `$XDG_CONFIG_HOME/termide/bookmarks.toml`)
- macOS: `~/Library/Application Support/termide/bookmarks.toml`
- Windows: `%APPDATA%\termide\bookmarks.toml`

### Пример конфигурации

```toml
[general]
theme = "windows-xp"
language = "auto"  # auto, bn, de, en, es, fr, hi, id, ja, ko, pt, ru, th, tr, vi, zh
vim_mode = false
project_retention_days = 30
bell_on_operation_complete = true
icon_mode = "auto"  # auto, emoji, unicode
always_detachable = false  # экземпляр переживает закрытие терминала (Unix)
resource_monitor_interval = 1000

[editor]
tab_size = 4
show_git_diff = true
word_wrap = true
auto_indent = true
auto_close_brackets = true

[file_manager]
extended_view_width = 50

[lsp]
enabled = true
auto_completion = true

[logging]
min_level = "info"
```

### Доступные темы

**Тёмные темы:**
- `windows-xp` - Тема по умолчанию (в стиле Windows XP)
- `dracula` - Популярная тема Dracula
- `monokai` - Классическая Monokai
- `nord` - Nord с синими тонами
- `onedark` - Atom One Dark
- `solarized-dark` - Тёмная Solarized
- `midnight` - Вдохновлено Midnight Commander
- `macos-dark` - Тёмный стиль macOS
- `ayu-dark` - Ayu Dark
- `billiard` - Зелёные тона бильярдного стола
- `catppuccin-macchiato` - Catppuccin Macchiato
- `everforest` - Тёмная Everforest
- `github-dark` - GitHub Dark
- `gruvbox` - Тёмная Gruvbox
- `kanagawa` - Kanagawa
- `material-ocean` - Material Ocean
- `rosepine` - Rosé Pine
- `tokyonight` - Tokyo Night

**Светлые темы:**
- `atom-one-light` - Atom One Light
- `ayu-light` - Ayu Light
- `github-light` - GitHub Light
- `manuscript` - Средневековый манускрипт в тонах состаренного пергамента
- `material-lighter` - Material Lighter
- `solarized-light` - Светлая Solarized
- `macos-light` - Светлый стиль macOS
- `blue-sky` - Blue Sky
- `green-backs` - Зелёные доллары
- `pinky-pie` - Pinky Pie

**Ретро-темы:**
- `far-manager` - Стиль FAR Manager
- `norton-commander` - Стиль Norton Commander
- `dos-navigator` - Стиль DOS Navigator
- `volkov-commander` - Стиль Volkov Commander
- `windows-95` - Стиль Windows 95
- `windows-98` - Стиль Windows 98

**Кинематографичные темы:**
- `matrix` - Цифровой дождь из «Матрицы» (зелёное на чёрном)
- `pip-boy` - Фосфорный CRT Pip-Boy 3000 из Fallout
- `terminator` - Эстетика HUD Skynet / марсианского красного

**Прочие темы:**
- `terminal` - Классический терминальный стиль (наследует цвета терминала)

**Примеры тем:**

| | | |
|:---:|:---:|:---:|
| ![Windows XP](assets/screenshots/themes/windows-xp.png) | ![Dracula](assets/screenshots/themes/dracula.png) | ![Ayu Light](assets/screenshots/themes/ayu-light.png) |
| Windows XP (по умолчанию) | Dracula | Ayu Light |
| ![Monokai](assets/screenshots/themes/monokai.png) | ![Nord](assets/screenshots/themes/nord.png) | ![Material Lighter](assets/screenshots/themes/material-lighter.png) |
| Monokai | Nord | Material Lighter |

### Пользовательские темы

Свои темы создаются размещением TOML-файлов в каталоге тем:
- Linux: `~/.config/termide/themes/`
- macOS: `~/Library/Application Support/termide/themes/`
- Windows: `%APPDATA%\termide\themes\`

Пользовательские темы имеют приоритет над встроенными с тем же именем. Примеры формата файла темы — в каталоге `crates/theme/themes/` репозитория.

### Пользовательские команды

Часто запускаемые команды оболочки описываются в `commands.toml` — глобальном,
в каталоге конфигурации, или `<проект>/.termide/commands.toml` для проекта — и
появляются в меню **Команды**:

```toml
[test]
name = "Run tests"
command = "cargo nextest run"
group = "cargo"
key = "Ctrl+Shift+T"

[clippy]
command = "cargo clippy --workspace -- -D warnings"
mode = "report"  # terminal (по умолчанию), background или report
```

`Команды → Добавить команду...` создаёт команду через форму. Режимы, параметры
и горячие клавиши описаны в [Пользовательских командах](doc/ru/actions.md).

## Разработка

Кодовая база — это Cargo workspace из модульных крейтов. Раскладку крейтов,
систему панелей и поток событий см. в
**[Руководстве разработчика](doc/ru/developer-guide.md)** и
**[Архитектуре](doc/ru/architecture.md)**.

### Сборка

```bash
# Отладочная сборка
cargo build

# Релизная сборка с оптимизациями
cargo build --release

# Запуск тестов
cargo test

# Проверка качества кода
cargo clippy
cargo fmt --check
```

### Разработка с Nix

В проекте есть Nix flake для воспроизводимых окружений разработки:

```bash
# Войти в shell разработки
nix develop

# Сборка через Nix
nix build

# Запуск проверок
nix flake check
```

## Вклад

Вклад приветствуется! Не стесняйтесь открывать issue и pull request'ы.

## Лицензия

Проект распространяется под лицензией MIT.

## Благодарности

Построено на:
- [ratatui](https://github.com/ratatui-org/ratatui) - Фреймворк терминального UI
- [crossterm](https://github.com/crossterm-rs/crossterm) - Кроссплатформенная работа с терминалом
- [portable-pty](https://github.com/wez/wezterm/tree/main/pty) - Реализация PTY
- [tree-sitter](https://github.com/tree-sitter/tree-sitter) - Подсветка синтаксиса
- [ropey](https://github.com/cessen/ropey) - Текстовый буфер
- [sysinfo](https://github.com/GuillaumeGomez/sysinfo) - Мониторинг системных ресурсов
