#!/usr/bin/env bash

set -u

# ============================================================
# Oxenna Terminal UI
# ============================================================

UI_DIR="${OXENNA_UI_DIR:-.oxenna-ui}"

LOG_FILE="$UI_DIR/output.log"
STATE_FILE="$UI_DIR/state"
ACTIVE_FILE="$UI_DIR/active"
PID_FILE="$UI_DIR/renderer.pid"

mkdir -p "$UI_DIR"
touch "$LOG_FILE"

# ============================================================
# Terminal
#
# IMPORTANT:
# When Make launches us in the background, stdin/stdout are
# not necessarily reliable for terminal-size detection.
#
# Always use /dev/tty.
# ============================================================

TTY="/dev/tty"

if [[ ! -e "$TTY" ]]; then
    echo "oxenna-ui: no controlling terminal" >&2
    exit 1
fi

# Make absolutely sure all UI output goes directly to the
# terminal, rather than through Make's stdout handling.
exec <"$TTY" >"$TTY" 2>"$TTY"


# ============================================================
# Colors
# ============================================================

RESET=$'\033[0m'
BOLD=$'\033[1m'

RED=$'\033[31m'
GREEN=$'\033[32m'
BLUE=$'\033[34m'
CYAN=$'\033[36m'
GRAY=$'\033[90m'


# ============================================================
# Terminal control
# ============================================================

enter_screen() {
    # Save/enter alternate screen.
    printf '\033[?1049h'

    # Hide cursor.
    printf '\033[?25l'

    # Clear screen.
    printf '\033[2J'
    printf '\033[H'
}

leave_screen() {
    # Restore normal terminal contents.
    printf '\033[?25h'
    printf '\033[?1049l'
}

cleanup() {
    rm -f "$ACTIVE_FILE"
    rm -f "$PID_FILE"

    leave_screen
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP


# ============================================================
# Terminal dimensions
# ============================================================

get_terminal_size() {
    local size

    # Read the size DIRECTLY from the terminal.
    size="$(stty size < "$TTY" 2>/dev/null || true)"

    if [[ "$size" =~ ^([0-9]+)[[:space:]]+([0-9]+)$ ]]; then
        TERM_ROWS="${BASH_REMATCH[1]}"
        TERM_COLS="${BASH_REMATCH[2]}"
    else
        # Fallback using ioctl through tput.
        TERM_ROWS="$(tput lines 2>/dev/null || printf '24')"
        TERM_COLS="$(tput cols 2>/dev/null || printf '80')"
    fi

    [[ "$TERM_ROWS" =~ ^[0-9]+$ ]] || TERM_ROWS=24
    [[ "$TERM_COLS" =~ ^[0-9]+$ ]] || TERM_COLS=80

    (( TERM_ROWS < 8 )) && TERM_ROWS=8
    (( TERM_COLS < 40 )) && TERM_COLS=40
}


# ============================================================
# Read state
#
# current|total|description
# ============================================================

read_state() {
    local state

    state="$(cat "$STATE_FILE" 2>/dev/null || true)"

    if [[ "$state" == *"|"* ]]; then
        IFS='|' read -r CURRENT TOTAL DESCRIPTION <<< "$state"
    else
        CURRENT=0
        TOTAL=1
        DESCRIPTION="Starting..."
    fi

    [[ -z "${CURRENT:-}" ]] && CURRENT=0
    [[ -z "${TOTAL:-}" ]] && TOTAL=1
    [[ -z "${DESCRIPTION:-}" ]] && DESCRIPTION="Working..."

    [[ "$CURRENT" =~ ^[0-9]+$ ]] || CURRENT=0
    [[ "$TOTAL" =~ ^[0-9]+$ ]] || TOTAL=1

    (( TOTAL == 0 )) && TOTAL=1
}


# ============================================================
# Draw helpers
# ============================================================

clear_line() {
    printf '\033[2K'
}


draw_rule() {
    local char="${1:--}"

    printf '%s' "$GRAY"

    printf '%*s' "$TERM_COLS" '' | tr ' ' "$char"

    printf '%s' "$RESET"
}


truncate() {
    local text="$1"
    local max="$2"

    if (( max <= 3 )); then
        printf '%s' "${text:0:$max}"
        return
    fi

    if (( ${#text} > max )); then
        printf '%s...' "${text:0:$((max - 3))}"
    else
        printf '%s' "$text"
    fi
}


# ============================================================
# Output window
# ============================================================

draw_output() {
    local rows="$1"

    local line
    local count=0

    mapfile -t lines < <(
        tail -n "$rows" "$LOG_FILE" 2>/dev/null
    )

    for line in "${lines[@]}"; do
        (( count++ ))

        # Remove CR used by programs that redraw their own line.
        line="${line//$'\r'/}"

        # Remove common ANSI colour/control sequences.
        line="$(printf '%s' "$line" | sed \
            $'s/\033\\[[0-9;?]*[[:alpha:]]//g')"

        # Don't let output wrap onto another terminal row.
        line="$(truncate "$line" $((TERM_COLS - 4)))"

        clear_line
        printf '  %s\n' "$line"
    done

    # Fill every remaining row.
    while (( count < rows )); do
        clear_line
        printf '\n'
        (( count++ ))
    done
}


# ============================================================
# Progress bar
# ============================================================

draw_progress() {
    local percent="$1"

    local width=$((TERM_COLS - 10))

    (( width < 10 )) && width=10

    local filled=$((width * percent / 100))
    local empty=$((width - filled))

    local left=""
    local right=""

    if (( filled > 0 )); then
        left="$(printf '%*s' "$filled" '' | tr ' ' '=')"
    fi

    if (( empty > 0 )); then
        right="$(printf '%*s' "$empty" '' | tr ' ' '-')"
    fi

    clear_line

    printf '  %s%s%s%s %3d%%%s\n' \
        "$GREEN" \
        "$left" \
        "$GRAY" \
        "$right" \
        "$percent" \
        "$RESET"
}


# ============================================================
# Render
# ============================================================

render() {
    get_terminal_size
    read_state

    local percent

    percent=$((CURRENT * 100 / TOTAL))

    (( percent < 0 )) && percent=0
    (( percent > 100 )) && percent=100

    # --------------------------------------------------------
    # EXACT SCREEN LAYOUT
    #
    # Row 1       Header
    # Row 2       Rule
    #
    # Row 3..N-4  Output
    #
    # Row N-3     Rule
    # Row N-2     Stage
    # Row N-1     Progress
    # Row N       Log
    #
    # Total = TERM_ROWS
    # --------------------------------------------------------

    local output_rows=$((TERM_ROWS - 6))

    (( output_rows < 1 )) && output_rows=1

    # Move to the absolute top-left.
    printf '\033[1;1H'

    # --------------------------------------------------------
    # Row 1
    # --------------------------------------------------------

    clear_line

    printf '%s%sOXENNA BUILD%s\n' \
        "$BOLD" \
        "$CYAN" \
        "$RESET"

    # --------------------------------------------------------
    # Row 2
    # --------------------------------------------------------

    clear_line
    draw_rule
    printf '\n'

    # --------------------------------------------------------
    # Output rows
    # --------------------------------------------------------

    draw_output "$output_rows"

    # --------------------------------------------------------
    # Footer row 1
    # --------------------------------------------------------

    clear_line
    draw_rule
    printf '\n'

    # --------------------------------------------------------
    # Footer row 2
    # --------------------------------------------------------

    local stage_width=$((TERM_COLS - 18))
    local stage

    stage="$(truncate "$DESCRIPTION" "$stage_width")"

    clear_line

    printf '  %s%s>%s  %-*s %s[%s/%s]%s\n' \
        "$BOLD" \
        "$BLUE" \
        "$RESET" \
        "$stage_width" \
        "$stage" \
        "$GRAY" \
        "$CURRENT" \
        "$TOTAL" \
        "$RESET"

    # --------------------------------------------------------
    # Footer row 3
    # --------------------------------------------------------

    draw_progress "$percent"

    # --------------------------------------------------------
    # Footer row 4
    # --------------------------------------------------------

    clear_line

    printf '%s  Output: %s%s' \
        "$GRAY" \
        "$LOG_FILE" \
        "$RESET"

    # VERY IMPORTANT:
    #
    # There is intentionally NO newline here.
    #
    # This is the final row of the terminal. Printing \n would
    # make the terminal scroll.
}


# ============================================================
# Start
# ============================================================

start() {
    local current="${1:-0}"
    local total="${2:-1}"
    local description="${3:-Starting...}"

    mkdir -p "$UI_DIR"

    printf '%s|%s|%s\n' \
        "$current" \
        "$total" \
        "$description" > "$STATE_FILE"

    : > "$LOG_FILE"

    touch "$ACTIVE_FILE"

    printf '%s\n' "$$" > "$PID_FILE"

    enter_screen

    # Draw immediately.
    render

    # Redraw frequently enough to feel smooth without
    # hammering the terminal.
    while [[ -f "$ACTIVE_FILE" ]]; do
        sleep 0.08
        render
    done
}


# ============================================================
# Update stage
# ============================================================

stage() {
    local current="${1:-0}"
    local total="${2:-1}"
    local description="${3:-Working...}"

    printf '%s|%s|%s\n' \
        "$current" \
        "$total" \
        "$description" > "$STATE_FILE"
}


# ============================================================
# Append output
# ============================================================

log() {
    shift || true

    printf '%s\n' "$*" >> "$LOG_FILE"
}


# ============================================================
# Stop
# ============================================================

stop() {
    rm -f "$ACTIVE_FILE"

    if [[ -f "$PID_FILE" ]]; then
        local pid

        pid="$(cat "$PID_FILE" 2>/dev/null || true)"

        if [[ -n "$pid" ]] &&
           [[ "$pid" != "$$" ]] &&
           kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
    fi
}


# ============================================================
# CLI
# ============================================================

case "${1:-}" in

    start)
        shift

        start \
            "${1:-0}" \
            "${2:-1}" \
            "${3:-Starting...}"
        ;;

    stage)
        shift

        stage \
            "${1:-0}" \
            "${2:-1}" \
            "${3:-Working...}"
        ;;

    log)
        log "$@"
        ;;

    stop)
        stop
        ;;

    render)
        render
        ;;

    *)
        echo "Usage:"
        echo
        echo "  $0 start CURRENT TOTAL DESCRIPTION"
        echo "  $0 stage CURRENT TOTAL DESCRIPTION"
        echo "  $0 log TEXT"
        echo "  $0 stop"
        echo "  $0 render"
        exit 2
        ;;

esac

