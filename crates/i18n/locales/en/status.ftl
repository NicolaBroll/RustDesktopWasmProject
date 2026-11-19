# Status bar: the lines of a message that are shown only on hover.
# $n: number of backup refs; $refs: their names, comma-separated.
status-backup = { $n ->
    [one] Backup
   *[other] Backups
}: { $refs }
# Status bar: context menu of the message, copies the whole text (with the backup refs).
status-copy = Copy message
