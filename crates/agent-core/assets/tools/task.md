---
snippet: delegate a self-contained task to another agent, which reports back
guideline.1: Set `background` when you have other work to do while the task runs, or when you are asked for a background agent; leave it out when your next step needs the answer.
guideline.2: A background task's answer arrives as a message of its own, later; go on with your work meanwhile, and do not poll for it with tools.
guideline.3: Set `panel` when the user should follow the task and go on with its session themselves — when asked to start an agent with a fresh context in a panel; leave it out for work only you need the answer of.
---
Delegate a self-contained task to another agent. It runs on its own — reading, searching and editing as its permissions allow, asking the user about the rest — and returns a final report. Use it to keep a focused sub-task and the files it touches out of this conversation. Give it everything it needs in the prompt; it does not see our history. With `background` the call returns at once while the task runs on; the agent's report comes to you later as a message, and `jobs` stops it by the id the call gave you once you no longer need it. With `panel` the task runs in the background in an agent panel of its own beside this one, which follows it while it runs and holds its session once it is over, for the user to go on with; its report still comes to you. Available agents:
{{agents}}
