#!/bin/bash
echo "Test stdin script started. Type messages and I'll echo them back."
while IFS= read -r line; do
    echo "You said: $line"
    if [ "$line" = "exit" ]; then
        echo "Exiting..."
        break
    fi
done
echo "Script finished."
