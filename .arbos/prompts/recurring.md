---
description: Does something over and over for ever 
argument-hint: needs a timer (when) I.e. do every minute
---

<task>
$ARGUMENTS
</task>

Makes a plan to run a job on a recurring schedule given the $ARGUMENTS. Try to parse the delay from the message directly, if the duration is not obvious, ask the user for the duration then kick off the job.
