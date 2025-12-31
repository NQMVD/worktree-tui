#!/bin/bash

# --- Configuration ---
# Your Discord Webhook URL (set as env var or edit here)
WEBHOOK_URL="${DISCORD_WEBHOOK:-'YOUR_WEBHOOK_HERE'}"
# The AI model to use, get a list with `opencode models [provider]`
# google/gemini-3-flash
# zai-coding-plan/glm-4.7
# opencode/minimax-m2.1-free
# WARNING: opencode doesnt exit if model is wrong but instead falls back to it's configured default!
MODEL=${AGENT_MODEL:-"opencode/minimax-m2.1-free"}
# Log file for the autonomous loop management
LOOP_LOG="autonomous_loop.log"

# The "Master Instruction" - This is the core prompt passed to the agent
# It tells the agent how to use the files and the bridge.
INITIAL_PROMPT="You are an autonomous TUI engineer.
1. Read README_AGENT.md for your operational protocol.
2. Check todo.md for your requirements.
3. Check agent_worklog.md for your current state.
4. Use ./tmux_bridge.sh to interact with a terminal.
5. Work on tasks until finished, then append 'MISSION_ACCOMPLISHED' to agent_worklog.md."

# --- Helper Functions ---
log_loop() {
    echo "[$(date +'%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOOP_LOG"
}

send_discord() {
    if [[ "$WEBHOOK_URL" == "YOUR_WEBHOOK_HERE" || -z "$WEBHOOK_URL" ]]; then return; fi
    curl -s -H "Content-Type: application/json" -X POST -d "{\"content\": \"$1\"}" "$WEBHOOK_URL" >/dev/null
}

cleanup_trap() {
    log_loop "Emergency exit: Script interrupted."
    send_discord "🚨 **Agent Loop Interrupted!** The process for model \`$MODEL\` has stopped unexpectedly."
    exit 1
}

# Trap Ctrl+C and script errors
trap cleanup_trap SIGINT SIGTERM ERR

# --- Initialization ---
if [ ! -f "agent_worklog.md" ]; then
    echo "# Agent Development Worklog" >agent_worklog.md
    log_loop "Initialized empty agent_worklog.md"
fi

log_loop "Starting autonomous loop with model: $MODEL"
send_discord "🚀 **Agent Loop Started**: Mission initiation for model \`$MODEL\`."

ITERATION=1

# --- The Autonomous Loop ---
while true; do
    log_loop "--- Starting Iteration $ITERATION ---"

    if [ "$ITERATION" -eq 1 ]; then
        # On the first iteration, we send the full mission prompt
        opencode run "$INITIAL_PROMPT" --model "$MODEL" --continue 2>&1 | tee -a "$LOOP_LOG"
    else
        # On subsequent runs (after crashes/timeouts), we force it to look at its own worklog
        opencode run "Resume your work. Read agent_worklog.md to determine where you left off. Continue until todo.md is complete." --model "$MODEL" --continue 2>&1 | tee -a "$LOOP_LOG"
    fi

    # Check for the exit signal written by the agent
    if grep -q "MISSION_ACCOMPLISHED" agent_worklog.md; then
        log_loop "Mission Accomplished signal detected in agent_worklog.md."
        break
    fi

    log_loop "Iteration $ITERATION ended. Cooling down before restart..."
    send_discord "♻️ **Iteration $ITERATION Complete**: Restarting the agent loop for model \`$MODEL\`."
    sleep 5
    ((ITERATION++))
done

log_loop "Autonomous mission complete."
send_discord "✅ **Mission Accomplished**: The agent has successfully completed all tasks and terminated the loop."
