---
snippet: run a program under the debugger: breakpoints, steps, stack, variables, expressions
guideline.1: Use `debug` to see what a program does at a line: set a breakpoint there, `start` it, then read the stack and variables, step and `evaluate` — rather than adding prints and rebuilding.
guideline.2: `start` takes a command with a debug adapter (`lldb-dap`, `gdb-dap`, `debugpy`, `dlv-dap`) from commands.toml, or a Python, Go or Rust file; a command-line debugger in a terminal is the user's to drive, not yours.
guideline.3: `stop` the session when you have what you came for; the program and the debug panel stay with the user otherwise.
---
Drive the debugger, as the user does in the debug panel. `action` says what to do:
- `start`: debug `command` (the identifier or label of a command with a `debugger` in commands.toml) or `file` (a Python script, the Go package in a file's directory, or the Cargo binary a Rust file belongs to), with the project's breakpoints set; waits until the program stops or ends, through any `build` first.
- `breakpoint`: set the breakpoint at `file`:`line` (1-based), or remove it with `set: false`; a running session gets it at once.
- `continue`, `step_over`, `step_in`, `step_out`: resume the stopped program and wait for the next stop or the end.
- `pause`: stop the running program where it is. `stop`: end the session and the program.
- `evaluate`: the value of `expression`, in the debugged language, in the selected frame.
- `frame`: select frame `frame` of the call stack (0 the innermost) to see its variables.
- `state`: the session as it stands.
Every action but `evaluate` returns the session as it then stands: where it stopped, the call stack (the selected frame marked `*`), the variables of the selected frame and the program's last output. Paths may be relative to the working directory.
