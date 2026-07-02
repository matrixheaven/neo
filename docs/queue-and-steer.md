# Message Queue & Message Steer

Long-running agent workflows (multi-step tool execution, goals, long reasoning)
block the main turn until completion. Neo provides two mechanisms to
communicate with the agent **while it is working**: the **message queue**
(follow-ups) and **message steer**.

## Quick reference

| Action                  | Key         | Behavior                                                    |
| ----------------------- | ----------- | ----------------------------------------------------------- |
| Queue a follow-up       | `Enter`     | While busy: queues the message; it is submitted next. |
| Steer the turn          | `Ctrl+S`    | Injects the message at the next natural break point.       |
| Edit last queued input  | `Alt+↑`     | Pulls the most recent queued follow-up back into composer. |
| (Idle) submit           | `Ctrl+S`    | When idle, Ctrl+S behaves like a normal submit.            |

## Pending Input Preview

While a turn is running, queued follow-ups and pending steers are shown in a
dedicated panel **above the composer**, not inside the transcript scrollback.
This keeps "what I already said" separate from "what is waiting to be sent"
and avoids cluttering the conversation history.

```text
──────────────────────────────────────────────────
   ❯ queued or steered message here
   Alt+↑ edit last queued message · Ctrl+S steer next
```

## Message Queue (follow-ups)

When the agent is mid-turn and you type a message and press `Enter`, the
message is **not** rejected. Instead it is appended to the follow-up queue and
shown in the Pending Input Preview panel.

- Follow-ups preserve **FIFO** order after the current turn's workflow drains.
- By default, Neo uses `follow_up_queue_mode = "All"`, so all currently queued
  follow-ups are submitted together in FIFO order. Set
  `follow_up_queue_mode = "OneAtATime"` to process one queued follow-up per
  follow-up turn.
- Slash commands cannot be queued — they must wait for the turn to finish.
- Press `Alt+↑` (or `↑` when the composer is empty and history is empty) to
  pull the most recent queued follow-up back into the composer for editing.

## Message Steer

Steering injects a message into the running turn at the **next natural break
point** — after a tool call finishes, after a thinking block ends, or at a
streaming boundary. The steer message becomes a context message for the model's
next decision, **without** interrupting the current step.

Press `Ctrl+S` to steer:

- If follow-ups are queued → the **oldest** queued follow-up is promoted to a
  steer (FIFO), one `Ctrl+S` press at a time.
- If no follow-up is queued and the composer has text → that text is sent as a
  steer and shown in the Pending Input Preview panel.
- If no turn is active → Ctrl+S falls back to a normal submit so the key is
  never dead.

### The Ctrl+S / XON/XOFF caveat

`Ctrl+S` is the terminal **XOFF** (stop) software flow-control character. Many
terminals swallow it by default, which freezes output until you press `Ctrl+Q`
(XON). If Ctrl+S does not reach Neo:

```bash
# Disable XON/XOFF flow control for the current terminal session
stty -ixon

# To make it permanent, add the above line to your ~/.zshrc or ~/.bashrc.
```

You can also rebind the steer action to any other key in your config:

```toml
# ~/.neo/config.toml
[tui.keybindings]
"tui.input.steer" = ["ctrl+g"]   # or any key sequence you prefer
```

## How it works (architecture)

- The controller pushes live input into a shared `SteerInputHandle`
  (`Arc<Mutex<VecDeque<ActiveTurnInput>>>`) threaded from `TurnChannels`
  through the streaming turn driver into `AgentRuntime`.
- `run_agent_turn` drains the handle at every step boundary (after each model
  turn, after each tool batch) and routes items into the existing
  `steering_queue` / `follow_up_queue` on `AgentContext` via `SteeringQueued` /
  `FollowUpQueued` events.
- The existing `drain_steering_queue` / `drain_follow_up_queue` machinery then
  injects the messages at the appropriate point — steering messages before the
  next model call, follow-ups as new turns.
- All queue events (`SteeringQueued`, `FollowUpQueued`, `QueueDrained`) are
  persisted to JSONL and replayed on `resume`, so queue state survives across
  sessions.
- This design is **append-only** and **prefix-cache friendly**: steer messages
  are added as context messages, never modifying history.

## Differences from a normal message

| Property      | Normal message (Enter, idle) | Queued follow-up (Enter, busy) | Steer (Ctrl+S, busy) |
| ------------- | ---------------------------- | ------------------------------ | -------------------- |
| When sent      | Starts turn immediately     | Starts turn after current one  | Injected mid-turn    |
| Interrupts?   | N/A                          | No                             | No                   |
| UI location   | Transcript (`✨` prefix)     | Pending Input Preview panel    | Pending Input Preview panel |
| Cache impact  | Fresh turn                  | Fresh turn                     | Append-only context  |
