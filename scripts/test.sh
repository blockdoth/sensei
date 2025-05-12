#!/usr/bin/env bash
set -e

cargo build 

mprocs --config scripts/mprocs.yaml