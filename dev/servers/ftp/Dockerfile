# vsftpd: plain FTP, explicit FTPS and implicit FTPS for the Gem integration tests.
# The self-signed certificate is generated at build time (test-only).
FROM alpine:3.22
RUN apk add --no-cache vsftpd openssl \
 && adduser -D -u 1000 -h /home/gem -s /bin/sh gem \
 && echo 'gem:gem-test-password' | chpasswd \
 && mkdir -p /etc/vsftpd/tls /var/run/vsftpd/empty \
 && openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj '/CN=gem-test-vsftpd' \
      -addext 'subjectAltName=DNS:localhost,IP:127.0.0.1' \
      -keyout /etc/vsftpd/tls/key.pem -out /etc/vsftpd/tls/cert.pem \
 && chmod 0600 /etc/vsftpd/tls/key.pem
COPY ftp/vsftpd.conf ftp/vsftpd-implicit.conf /etc/vsftpd/
COPY ftp/entrypoint.sh /usr/local/bin/entrypoint.sh
EXPOSE 21 990
CMD ["/usr/local/bin/entrypoint.sh"]
