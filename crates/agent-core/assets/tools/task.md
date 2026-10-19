---
snippet: delegate a self-contained task to another agent, which reports back
guideline.1: Set `background` when you have other work to do while the task runs, or when you are asked for a background agent; leave it out when your next step needs the answer.
guideline.2: A background task's answer arrives as a message of its own, later; go on with your work meanwhile, and do not poll for it with tools.
---
Delegate a self-contained task to another agent. It runs on its own — reading, searching and editing as its permissions allow, asking the user about the rest — and returns a final report. Use it to keep a focused sub-task and the files it touches out of this conversation. Give it everything it needs in the prompt; it does not see our history. With `background` the call returns at once while the task runs on; the agent's report comes to you later as a message. Available agents:
{{agents}}
