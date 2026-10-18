//! Debugging support for termide: the breakpoints set in editors, kept and
//! saved per file, and the command lines that start a command-line debugger
//! (lldb, gdb, Delve, pdb) in a terminal with those breakpoints set.

pub mod breakpoints;
pub mod cargo;
pub mod dap;
pub mod execution;
mod launch;
mod process;

pub use launch::{shell_quote, Breakpoint, Debugger, Launch};
pub use process::detach;
