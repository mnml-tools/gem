# ProFTPD: plain FTP and explicit FTPS with MLSD/MLST, for the Gem integration tests.
# The self-signed certificate is generated at build time (test-only).
FROM alpine:3.22
RUN apk add --no-cache proftpd proftpd-mod_tls openssl \
 && adduser -D -u 1000 -h /home/gem -s /bin/sh gem \
 && echo 'gem:gem-test-password' | chpasswd \
 && mkdir -p /etc/proftpd/tls /run/proftpd \
 && openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj '/CN=gem-test-proftpd' \
      -addext 'subjectAltName=DNS:localhost,IP:127.0.0.1' \
      -keyout /etc/proftpd/tls/key.pem -out /etc/proftpd/tls/cert.pem \
 && chmod 0600 /etc/proftpd/tls/key.pem
COPY proftpd/proftpd.conf /etc/proftpd/proftpd.conf
EXPOSE 21
CMD ["proftpd", "--nodaemon", "--config", "/etc/proftpd/proftpd.conf"]
