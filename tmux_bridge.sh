#!/bin/bash

# Configuration
DEFAULT_WIDTH=100
DEFAULT_HEIGHT=30
FONT_FAMILY="BerkeleyMonoVariable Nerd Font"

# Directory Setup
LOG_DIR="logs"
SCREENSHOT_DIR="screenshots"
mkdir -p "$LOG_DIR" "$SCREENSHOT_DIR"

# Help / Usage
usage() {
    echo "Usage: $0 {command} <session_name> [args]"
    echo "Commands:"
    echo "  start <session> <cmd> [width] [height]  Starts TUI with interrupted log check"
    echo "  send <session> <keys>                   Sends keys to specific session"
    echo "  capture <session>                       Captures plain text screen"
    echo "  capture-ansi <session>                  Captures screen with ANSI colors"
    echo "  cursor <session>                        Returns X,Y cursor position"
    echo "  inspect <session>                       Returns Cursor + Plain Screen"
    echo "  recover <session>                       Reads last 20 lines of most recent INTERRUPTED log"
    echo "  screenshot <session> [filename]         Renders screen to PNG using freeze"
    echo "  check-deps                              Verifies tmux, freeze, and font availability"
    echo "  stop <session>                          Stops session and rotates logs"
    echo "  list                                    Lists all tmux sessions"
    echo "  wait <seconds>                          Sleeps for the specified time"
}

ACTION=$1
SESSION_NAME=$2

# Validation: Session name is mandatory for most commands
if [[ -z "$SESSION_NAME" && "$ACTION" != "list" && "$ACTION" != "wait" && "$ACTION" != "check-deps" ]]; then
    echo "Error: session_name is mandatory for action '$ACTION'"
    usage
    exit 1
fi

# Define Log File Path in the logs/ directory
LOG_FILE="$LOG_DIR/agent_interaction_${SESSION_NAME}.log"

log() {
    if [[ "$ACTION" != "list" && "$ACTION" != "wait" && "$ACTION" != "check-deps" ]]; then
        echo "[$(date +'%Y-%m-%d %H:%M:%S')] $1" >>"$LOG_FILE"
    fi
}

# Check dependencies helper
check_fonts() {
    echo "Checking for $FONT_FAMILY..."
    if command -v fc-list &>/dev/null; then
        if fc-list : family | grep -iq "$FONT_FAMILY"; then
            echo "Success: $FONT_FAMILY found via fc-list."
            return 0
        fi
    fi

    # macOS specific fallback if fontconfig isn't installed
    if [[ "$OSTYPE" == "darwin"* ]]; then
        if system_profiler SPFontsDataType | grep -iq "$FONT_FAMILY"; then
            echo "Success: $FONT_FAMILY found via system_profiler."
            return 0
        fi
    fi
    echo "Error: $FONT_FAMILY not detected."
    return 1
}

case "$ACTION" in
check-deps)
    echo "Verifying environment..."
    command -v tmux >/dev/null && echo "tmux: Installed" || echo "tmux: MISSING"
    command -v freeze >/dev/null && echo "freeze: Installed" || echo "freeze: MISSING"
    check_fonts
    ;;

start)
    COMMAND=$3
    WIDTH=${4:-$DEFAULT_WIDTH}
    HEIGHT=${5:-$DEFAULT_HEIGHT}

    if [ -z "$COMMAND" ]; then
        echo "Error: No command specified to run."
        exit 1
    fi

    if [ -f "$LOG_FILE" ]; then
        TIMESTAMP=$(date +'%Y%m%d_%H%M%S')
        INTERRUPTED_LOG="$LOG_DIR/agent_interaction_${SESSION_NAME}_INTERRUPTED_${TIMESTAMP}.log"
        mv "$LOG_FILE" "$INTERRUPTED_LOG"
        echo "Warning: Previous session did not exit cleanly. Log archived to $INTERRUPTED_LOG"
    fi

    if tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
        echo "Error: Session '$SESSION_NAME' already exists in tmux. Use 'stop' first."
        exit 1
    else
        tmux new-session -d -s "$SESSION_NAME" -x "$WIDTH" -y "$HEIGHT" "$COMMAND"
        log "STARTED session '$SESSION_NAME' (${WIDTH}x${HEIGHT}) with command: $COMMAND"
        echo "Started session '$SESSION_NAME' at ${WIDTH}x${HEIGHT} running: $COMMAND"
    fi
    ;;

recover)
    LATEST_INTERRUPTED=$(ls -t "$LOG_DIR"/agent_interaction_${SESSION_NAME}_INTERRUPTED_*.log 2>/dev/null | head -n 1)
    if [ -z "$LATEST_INTERRUPTED" ]; then
        echo "No interrupted logs found for session '$SESSION_NAME'."
    else
        echo "--- LAST 20 LINES OF $LATEST_INTERRUPTED ---"
        tail -n 20 "$LATEST_INTERRUPTED"
    fi
    ;;

screenshot)
    if ! tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
        echo "Error: Session '$SESSION_NAME' does not exist."
        exit 1
    fi
    if ! command -v freeze &>/dev/null; then
        echo "Error: 'freeze' is not installed."
        exit 1
    fi

    TIMESTAMP=$(date +'%Y%m%d_%H%M%S')
    FILE_NAME=${3:-"screenshot_${SESSION_NAME}_${TIMESTAMP}.png"}
    OUTPUT_PATH="$SCREENSHOT_DIR/$FILE_NAME"

    tmux capture-pane -e -p -t "$SESSION_NAME" | freeze --font.family "$FONT_FAMILY" --output "$OUTPUT_PATH"
    log "SCREENSHOT saved to $OUTPUT_PATH"
    echo "Screenshot saved to $OUTPUT_PATH"
    ;;

send)
    KEYS=$3
    if [ -z "$KEYS" ]; then
        echo "Error: No keys specified."
        exit 1
    fi
    tmux send-keys -t "$SESSION_NAME" "$KEYS"
    log "SEND :: $KEYS"
    ;;

capture)
    if ! tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
        echo "Error: Session '$SESSION_NAME' does not exist."
        exit 1
    fi
    tmux capture-pane -p -t "$SESSION_NAME"
    log "CAPTURED"
    ;;

capture-ansi)
    if ! tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
        echo "Error: Session '$SESSION_NAME' does not exist."
        exit 1
    fi
    tmux capture-pane -e -p -t "$SESSION_NAME"
    log "CAPTURED ANSI"
    ;;

cursor)
    if ! tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
        echo "Error: Session '$SESSION_NAME' does not exist."
        exit 1
    fi
    tmux display-message -t "$SESSION_NAME" -p "#{cursor_x},#{cursor_y}"
    log "GOT CURSOR POSITION"
    ;;

inspect)
    echo "CURSOR: $(tmux display-message -t "$SESSION_NAME" -p "#{cursor_x},#{cursor_y}")"
    echo "SCREEN:"
    tmux capture-pane -p -t "$SESSION_NAME"
    log "INSPECTED (CURSOR + SCREEN)"
    ;;

stop)
    if ! tmux has-session -t "$SESSION_NAME" 2>/dev/null; then
        echo "Error: Session '$SESSION_NAME' does not exist."
        exit 1
    fi

    tmux kill-session -t "$SESSION_NAME"
    log "STOPPED session '$SESSION_NAME'"
    echo "Stopped session '$SESSION_NAME'"

    if [ -f "$LOG_FILE" ]; then
        TIMESTAMP=$(date +'%Y%m%d_%H%M%S')
        mv "$LOG_FILE" "$LOG_DIR/agent_interaction_${SESSION_NAME}_${TIMESTAMP}.log"
    fi
    ;;

list)
    tmux list-sessions
    ;;

wait)
    SLEEP_TIME=${2:-1}
    log "WAIT for $SLEEP_TIME"
    sleep "$SLEEP_TIME"
    ;;

*)
    usage
    exit 1
    ;;
esac
