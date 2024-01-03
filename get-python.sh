#!/bin/bash

cargo install pyoxidizer || true

if [ -z "$1" ]; then
	pyoxidizer generate-python-embedding-artifacts \
		runners/python_runner/pyembedded
else
	pyoxidizer generate-python-embedding-artifacts \
		--target-triple $1 \
		runners/python_runner/pyembedded
fi


