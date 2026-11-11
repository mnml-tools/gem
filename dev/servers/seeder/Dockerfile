# One-shot container that fills the shared `home` volume with the test tree.
FROM alpine:3.22
COPY seeder/seed.sh /usr/local/bin/seed.sh
COPY seed /seed
CMD ["/usr/local/bin/seed.sh"]
