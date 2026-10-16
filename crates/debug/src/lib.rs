//! Debugging support for termide: the breakpoints set in editors, kept and
//! saved per file, and the command lines that start a command-line debugger
//! (lldb, gdb, Delve, pdb) in a terminal with those breakpoints set.

pub mod breakpoints;
mod launch;

pub use launch::{shell_quote, Breakpoint, Debugger, Launch};
