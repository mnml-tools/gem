# Synthetic Makefile.
.PHONY: build test clean

BIN := dist/api

build: $(BIN)

$(BIN): $(wildcard *.go)
	mkdir -p dist
	go build -o $@ .

test:
	go test ./...

clean:
	rm -rf dist
