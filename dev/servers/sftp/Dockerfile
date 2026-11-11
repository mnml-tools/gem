# OpenSSH SFTP server for the Gem integration tests. Test-only credentials.
FROM alpine:3.22
RUN apk add --no-cache openssh-server openssh-sftp-server \
 && adduser -D -u 1000 -h /home/gem -s /bin/sh gem \
 && echo 'gem:gem-test-password' | chpasswd \
 && ssh-keygen -A \
 && mkdir -p /etc/ssh/authorized_keys
COPY sftp/sshd_config /etc/ssh/sshd_config
COPY keys/authorized_keys /etc/ssh/authorized_keys/gem
RUN chmod 0644 /etc/ssh/authorized_keys/gem
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
