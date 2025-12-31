#!/bin/bash

# --- Configuration ---
WEBHOOK_URL="${DISCORD_WEBHOOK:-'YOUR_DISCORD_WEBHOOK_URL_HERE'}"
# Using the GLM-4.7 model as requested
MODEL="custom:glm-4.7"
LOOP_LOG="logs/autonomous_loop.log"
# Set to 'medium' for safer local development
AUTONOMY="--auto medium"

INITIAL_PROMPT="Read README_AGENT.md. Check todo.md, update agent_worklog.md, and proceed.
Use ./tmux_bridge.sh for TUI interaction. Commit changes to git when verified.
Append MISSION_ACCOMPLISHED to agent_worklog.md when finished."

mkdir -p logs screenshots

log_loop() {
    echo "[$(date +'%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOOP_LOG"
}

send_discord() {
    if [[ "$WEBHOOK_URL" == "YOUR_DISCORD_WEBHOOK_URL_HERE" || -z "$WEBHOOK_URL" ]]; then return; fi
    curl -s -H "Content-Type: application/json" -X POST -d "{\"content\": \"$1\"}" "$WEBHOOK_URL" >/dev/null
}

cleanup_trap() {
    log_loop "Emergency exit: Script interrupted."
    send_discord "🚨 **Droid Loop Stopped!** Process for \`$MODEL\` was interrupted."
    exit 1
}
trap cleanup_trap SIGINT SIGTERM ERR

[[ ! -f "agent_worklog.md" ]] && echo "# Agent Development Worklog" >agent_worklog.md

log_loop "Starting Droid loop with GLM-4.7 (Medium Autonomy)"
send_discord "🚀 **Droid Loop Started**: Mission initiation for \`$MODEL\`."

ITERATION=1
SESSION_ID=""

while true; do
    log_loop "--- Iteration $ITERATION ---"

    # Decide between initial prompt or resume
    if [[ -z "$SESSION_ID" ]]; then
        CURRENT_PROMPT="$INITIAL_PROMPT"
        RESUME_ARG=""
    else
        CURRENT_PROMPT="Continue work based on agent_worklog.md"
        RESUME_ARG="-s $SESSION_ID"
    fi

    log_loop "Running droid with model $MODEL..."
    log_loop "Command: droid exec $AUTONOMY --model \"$MODEL\" $RESUME_ARG -o json \"$CURRENT_PROMPT\""
    # Run droid and capture JSON output to extract session_id
    # We use jq to parse the session_id from the JSON response
    RESPONSE=$(droid exec $AUTONOMY --model "$MODEL" $RESUME_ARG -o json "$CURRENT_PROMPT" 2>&1)

    # Log the raw response for debugging if needed
    echo "$RESPONSE" >>"$LOOP_LOG"

    # extract result message for logging
    RESULT_MESSAGE=$(echo "$RESPONSE" | jq -r '.result // "No message returned."')
    log_loop "Droid Response Message: $RESULT_MESSAGE"

    # extract more stats for logging
    TURNS_TAKEN=$(echo "$RESPONSE" | jq -r '.num_turns // 0')
    TIME_TAKEN=$(echo "$RESPONSE" | jq -r '.duration_ms // 0')
    TIME_TAKEN=$((TIME_TAKEN / 1000 / 60)) # convert ms to minutes
    log_loop "Droid Stats: Turns: ${TURNS_TAKEN}, Time Taken: ${TIME_TAKEN}mins"

    # Update Session ID from JSON output
    NEW_ID=$(echo "$RESPONSE" | jq -r '.session_id // empty')
    [[ -n "$NEW_ID" ]] && SESSION_ID="$NEW_ID"

    if grep -q "MISSION_ACCOMPLISHED" agent_worklog.md; then
        log_loop "Mission Accomplished detected."
        break
    fi

    log_loop "Iteration $ITERATION ended. Cooling down..."
    send_discord "♻️ **Iteration $ITERATION Complete**: Restarting the droid agent loop for model \`$MODEL\`."
    sleep 5
    ((ITERATION++))
done

send_discord "✅ **Mission Accomplished**: Droid has terminated the loop."

# send the worklog.md as text to discord
WORKLOG_CONTENT=$(cat agent_worklog.md | head -n 100) # limit to first 100 lines
send_discord "📝 **Final Agent Worklog:**\n\n$WORKLOG_CONTENT\n"
