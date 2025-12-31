#!/bin/bash

# --- Configuration ---
WEBHOOK_URL="${DISCORD_WEBHOOK:-'YOUR_DISCORD_WEBHOOK_URL_HERE'}"
LOOP_LOG="logs/autonomous_loop.log"

# Re-enabling verbose and adding --debug to find out why it hangs.
CLAUDE_OPTS="--print --permission-mode dontAsk --tools Bash,Read,Write,Edit,Glob,Grep --no-session-persistence --verbose"

INITIAL_PROMPT="Read README_AGENT.md. Check todo.md, update agent_worklog.md, and proceed. Use ./tmux_bridge.sh for TUI interaction. Append MISSION_ACCOMPLISHED to agent_worklog.md when finished."

mkdir -p logs screenshots

log_loop() {
	echo "[$(date +'%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOOP_LOG"
}

send_discord() {
	if [[ "$WEBHOOK_URL" == "YOUR_DISCORD_WEBHOOK_URL_HERE" || -z "$WEBHOOK_URL" ]]; then return; fi
	local payload=$(jq -n --arg content "$1" '{"content": $content}')
	curl -s -H "Content-Type: application/json" -X POST -d "$payload" "$WEBHOOK_URL" >/dev/null
}

cleanup_trap() {
	log_loop "Emergency exit: Script interrupted."
	send_discord "🚨 **Claude Loop Stopped!** Process was interrupted."
	exit 1
}
trap cleanup_trap SIGINT SIGTERM ERR

log_loop "Starting Claude loop (DEBUG MODE ENABLED)"
send_discord "🚀 **Claude Loop Started**: Mission initiation."

ITERATION=1

while true; do
	log_loop "--- Iteration $ITERATION ---"

	if [[ $ITERATION -eq 1 ]]; then
		CURRENT_PROMPT="$INITIAL_PROMPT"
		EXTRA_ARGS=""
	else
		CURRENT_PROMPT="Continue your work. Read agent_worklog.md to see where you left off. Continue until MISSION_ACCOMPLISHED is appended."
		EXTRA_ARGS="--continue"
	fi

	log_loop "Running Claude..."
	echo "--- CLAUDE RESPONSE START ---" >>"$LOOP_LOG"

	# We set ANTHROPIC_LOG=debug to see internal API calls in the log.
	(
		export SHELL=/bin/sh
		export PS1="$ "
		export ANTHROPIC_LOG=debug
		export GIT_OPTIONAL_LOCKS=0
		export GIT_TERMINAL_PROMPT=0
		claude $CLAUDE_OPTS $EXTRA_ARGS "$CURRENT_PROMPT" 2>&1
	) | tee -a "$LOOP_LOG"

	echo "--- CLAUDE RESPONSE END ---" >>"$LOOP_LOG"

	if grep -q "MISSION_ACCOMPLISHED" agent_worklog.md; then
		log_loop "Mission Accomplished detected in agent_worklog.md."
		break
	fi

	log_loop "Iteration $ITERATION ended. Cooling down before restart..."
	send_discord "♻️ **Iteration $ITERATION Complete**: Restarting the Claude loop."
	sleep 5
	((ITERATION++))
done

send_discord "✅ **Mission AccomplISHED**: Claude has terminated the loop."
