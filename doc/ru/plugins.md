# Плагины

Плагин — это программа, которую termide запускает в фоне, пока открыт проект,
и с которой общается по JSON-RPC через её stdin и stdout. Плагин может
реагировать на открытие и сохранение файлов, добавлять команды в палитру
команд и меню «Команды», показывать элементы в строке состояния, открывать собственные панели и давать
инструменты агенту.

Плагины глобальные: они устанавливаются в каталог конфигурации, а проект может
только выключить установленный вами плагин или включить тот, которому вы это
разрешили. Открытый репозиторий никогда не решает, какая программа запустится.

## Установка

Скопируйте каталог плагина в `plugins/` рядом с вашим `config.toml`:

| Платформа | Каталог |
|---|---|
| Linux | `~/.config/termide/plugins/<имя>/` |
| macOS | `~/Library/Application Support/termide/plugins/<имя>/` |
| Windows | `%APPDATA%\termide\plugins\<имя>\` |

Имя каталога — это имя плагина: латинские буквы, цифры, `-` и `_`. В каталоге
лежит `plugin.toml`:

```toml
description = "Форматирует при сохранении"
command = "./run.py"          # относительно этого каталога
args = ["--quiet"]
[env]
LOG_LEVEL = "info"
```

`command` — абсолютный путь, путь относительно каталога плагина (содержит `/`)
или просто имя, например `python3`. Просто имя ищется только в абсолютных
элементах `PATH`. Плагин запускается с рабочим каталогом, равным своему
каталогу, а корень проекта узнаёт из `initialize`.

Плагины читаются при запуске termide. После установки или удаления плагина
перезапустите termide.

## Включение и выключение

Глобальный `config.toml` задаёт режим каждого плагина:

```toml
[plugins.formatter]
mode = "on"       # по умолчанию: работает во всех проектах, проект может выключить
[plugins.deploy]
mode = "opt-in"   # работает только в проектах, которые его включили
[plugins.experiment]
mode = "off"      # не работает нигде

[plugins.formatter.settings]
line_width = 100  # передаётся плагину в `initialize`
```

Проектный `.termide/config.toml` может переключить плагин в пределах его
режима и дополнить его настройки:

```toml
[plugins.deploy]
enabled = true
[plugins.formatter]
enabled = false
```

| Глобальный `mode` | Проект молчит | `enabled = false` | `enabled = true` |
|---|---|---|---|
| `on` (или нет записи) | работает | выключен | работает |
| `opt-in` | выключен | выключен | работает |
| `off` | выключен | выключен | выключен |

`mode` в проектном файле и `enabled` в глобальном игнорируются с
предупреждением в журнале. Ставьте `opt-in` плагинам, которые работают с
содержимым проекта (например, запускают его скрипты), чтобы ни один
репозиторий не включил их без вашего согласия.

У каждого открытого проекта свой экземпляр каждого плагина, в том числе у
проектов, отложенных в фон. Упавший плагин перезапускается через 1, 2, 4…
секунды. После трёх падений за минуту он остаётся выключенным, пока не
изменятся его настройки или termide не перезапустится; строка состояния
сообщает об этом.

## Как написать плагин

Каждое сообщение — одна строка JSON-RPC 2.0. Всё, что плагин пишет в stderr,
попадает в журнал termide.

**termide → плагин**

| Метод | Вид | Параметры | Результат |
|---|---|---|---|
| `initialize` | запрос | `protocolVersion` (1), `termide.version`, `plugin.name`, `plugin.dir`, `project.root`, `settings` | что плагин добавляет, см. ниже |
| `initialized` | уведомление | | |
| `file/opened`, `file/saved` | уведомление, только при подписке | `path` | |
| `command/execute` | запрос | `id` | любой |
| `tool/call` | запрос | `name`, `arguments` | `text`, `isError` |
| `shutdown` | запрос | | `null` |
| `exit` | уведомление | | |

Результат `initialize` и параметры уведомления `contributions/changed`,
которым плагин позже заменяет его:

```json
{
  "subscriptions": ["file/saved"],
  "commands": [{ "id": "check", "title": "Проверить проект" }],
  "tools": [{
    "name": "lookup",
    "description": "Найти слово в глоссарии проекта",
    "inputSchema": { "type": "object", "properties": { "word": { "type": "string" } } },
    "readOnly": true
  }]
}
```

Команды появляются как `<плагин>: <название>` в палитре команд (категория
«Плагины») и в конце меню «Команды». Инструменты предлагаются сессиям агента в проекте
плагина под именем `plugin-<плагин>__<инструмент>`. Правила разрешений агента
относятся к ним как к инструментам MCP: перед запуском спрашивают, если
инструмент не `readOnly`.

**плагин → termide**

| Метод | Вид | Параметры |
|---|---|---|
| `contributions/changed` | уведомление | как результат `initialize` |
| `window/notify` | запрос | `message`, `level` (`"info"` или `"error"`) |
| `status/set` | запрос | `id`, `text` (пустой текст убирает элемент) |

Плагин проекта, отложенного в фон, на экран не выводится: его уведомления
попадают в журнал.

### Панели

Плагин может открывать собственные панели, нарисованные по присланному им
Markdown: заголовки, списки, таблицы, код и ссылки — как в просмотрщике
Markdown, с курсором, выделением, поиском и копированием. Ссылка на
`plugin:<действие>` — это кнопка: переход по ней (`Enter` на ссылке или щелчок)
отправляет действие обратно плагину, а тот отвечает обновлением панели.

| Метод | Вид | Направление | Параметры / результат |
|---|---|---|---|
| `panel/open` | запрос | плагин → termide | `title`, `markdown`; результат — `{ "panel" }`, её id |
| `panel/update` | запрос | плагин → termide | `panel`, `markdown`, `title` (необязательно) |
| `panel/close` | запрос | плагин → termide | `panel` |
| `panel/action` | уведомление | termide → плагин | `panel`, `action` |
| `panel/closed` | уведомление | termide → плагин | `panel`: пользователь её закрыл |

Панель открывается в проекте плагина, и он должен быть текущим; в раскладке
она не сохраняется. Обновление оставляет читателя на месте, насколько хватает
нового содержимого. Когда плагин останавливается, его панели остаются и
сообщают об этом; запущенный заново плагин их не получает.

```python
# в цикле примера ниже
elif method == "command/execute":            # команда «Показать отчёт»
    send({"jsonrpc": "2.0", "id": id, "result": None})
    send({"jsonrpc": "2.0", "id": "open", "method": "panel/open",
          "params": {"title": "Отчёт", "markdown": "# Отчёт\n\n[Обновить](plugin:refresh)"}})
elif msg.get("id") == "open":                # ответ на panel/open
    panel = msg["result"]["panel"]
elif method == "panel/action" and msg["params"]["action"] == "refresh":
    send({"jsonrpc": "2.0", "id": "upd", "method": "panel/update",
          "params": {"panel": panel, "markdown": "# Отчёт\n\nОбновлено.\n\n[Обновить](plugin:refresh)"}})
```

### Пример

Плагин на Python, который считает сохранения в строке состояния:

```python
#!/usr/bin/env python3
import json, sys

def send(message):
    print(json.dumps(message), flush=True)

saves = 0
for line in sys.stdin:
    msg = json.loads(line)
    method, id = msg.get("method"), msg.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id, "result": {
            "subscriptions": ["file/saved"],
            "commands": [{"id": "reset", "title": "Сбросить счётчик"}]}})
    elif method == "file/saved":
        saves += 1
        send({"jsonrpc": "2.0", "id": f"s{saves}", "method": "status/set",
              "params": {"id": "saves", "text": f"сохранено {saves}×"}})
    elif method == "command/execute":
        saves = 0
        send({"jsonrpc": "2.0", "id": id, "result": None})
        send({"jsonrpc": "2.0", "id": "r", "method": "status/set",
              "params": {"id": "saves", "text": ""}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": id, "result": None})
    elif method == "exit":
        break
```

Сохраните его как `plugins/save-count/run.py`, сделайте исполняемым и добавьте
`plugins/save-count/plugin.toml` со строкой `command = "./run.py"`.
