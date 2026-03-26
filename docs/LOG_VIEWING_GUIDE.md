# Log Viewing Guide

## Issue Fixed
Previously, users could not see logs from the second server because the TUI interface didn't make it clear that logs are displayed per selected service and that navigation is required to view different services' logs.

## Solution
The UI has been enhanced to make service navigation and log viewing more intuitive:

1. **Services Panel Title**: Updated from "Services (↑/↓ select, ...)" to "Services (↑/↓ select & view logs, ...)" to clearly indicate that navigation changes which logs are displayed.

2. **Logs Panel Title**: Changed from static "Logs" to dynamic "Logs - [service-name]" to show exactly which service's logs are currently being displayed.

## How to View Logs from Multiple Services

1. Run the example: `./run_example.sh`
2. In the TUI interface:
   - Use **↑/↓ arrow keys** to navigate between services in the left panel
   - The right panel will show logs for the currently selected service
   - The logs panel title will show "Logs - example-service-1" or "Logs - example-service-2"
   - Press **Enter** or **Space** to start/stop the selected service
   - Press **r** to restart the selected service
   - Press **c** to clear logs for the selected service
   - Press **q** to quit

## Expected Behavior
- **example-service-1**: Shows logs like "Hello #0 from service 1", "Hello #1 from service 1", etc.
- **example-service-2**: Shows logs like "Hello #0 from service 2", "Hello #1 from service 2", etc.

Both services log every 5 seconds, so you should see different logs when switching between them using the arrow keys.

## Testing the Fix
1. Start both services using Enter/Space
2. Navigate to service 1 with arrow keys - you should see "Logs - example-service-1" and logs from service 1
3. Navigate to service 2 with arrow keys - you should see "Logs - example-service-2" and logs from service 2
4. The interface now makes it clear that you're viewing service-specific logs and how to switch between them