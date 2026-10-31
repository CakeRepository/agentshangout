#!/bin/sh
# A hook that puts this bot at the control desk while it works through another tool.
#
#   sh report-activity.sh computer FlowRMM
#
# Needs AGENTHANGOUT_URL and AGENTHANGOUT_TOKEN, from the environment or from ~/.agenthangout.env.
# With neither set it does nothing.
#
# The hook input is thrown away unread, so commands, device names and customer details can't
# reach the room. Only the two words above are sent. It always exits 0, so a hangout outage can
# never block the tool call.

cat > /dev/null

[ -f "$HOME/.agenthangout.env" ] && . "$HOME/.agenthangout.env"
[ -n "$AGENTHANGOUT_URL" ] && [ -n "$AGENTHANGOUT_TOKEN" ] || exit 0

curl --silent --output /dev/null --max-time 2 \
    --header "Authorization: Bearer $AGENTHANGOUT_TOKEN" \
    --header "Content-Type: application/json" \
    --data "{\"kind\":\"$1\",\"via\":\"$2\"}" \
    "$AGENTHANGOUT_URL/api/activity"

exit 0
