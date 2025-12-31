#!/bin/bash

# check deps first
./tmux_bridge.sh check-deps

SESSION_NAME="test_session"

# start session with wtt
./tmux_bridge.sh start "$SESSION_NAME" wtt

# wait a moment to ensure the session is started
./tmux_bridge.sh wait 1

# open the help menu
./tmux_bridge.sh send "$SESSION_NAME" "?"

# capture the current pane content
./tmux_bridge.sh capture "$SESSION_NAME" >help_menu.txt
# and take a screenshot with ansi colors
./tmux_bridge.sh screenshot "$SESSION_NAME"

# exit the help menu
./tmux_bridge.sh send "$SESSION_NAME" "Esc"
# and exit wtt gracefully
./tmux_bridge.sh send "$SESSION_NAME" "q"

# stop the session
./tmux_bridge.sh stop "$SESSION_NAME"
