#!/bin/bash

if ! command -v pyoxidizer >/dev/null 2>&1; then
	cargo install pyoxidizer
fi

if [ -z "$1" ]; then
	pyoxidizer generate-python-embedding-artifacts \
		runners/python_runner/pyembedded
else
	pyoxidizer generate-python-embedding-artifacts \
		--target-triple $1 \
		runners/python_runner/pyembedded
fi


