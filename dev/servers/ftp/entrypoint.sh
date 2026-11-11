#!/bin/sh
# Runs two vsftpd instances: plain/explicit FTPS on 21 and implicit FTPS on 990. Neither may be
# PID 1 (vsftpd crashes when it reaps a child it did not start), so this shell supervises both
# and exits when either stops; compose runs it under `init: true`.
set -u
touch /var/log/vsftpd.log
tail -F /var/log/vsftpd.log &
vsftpd /etc/vsftpd/vsftpd.conf &
explicit=$!
vsftpd /etc/vsftpd/vsftpd-implicit.conf &
implicit=$!
trap 'kill $explicit $implicit 2>/dev/null; exit 0' TERM INT
while kill -0 "$explicit" 2>/dev/null && kill -0 "$implicit" 2>/dev/null; do
  sleep 1
done
echo "vsftpd stopped" >&2
kill "$explicit" "$implicit" 2>/dev/null
exit 1
