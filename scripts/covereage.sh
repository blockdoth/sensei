#!/usr/bin/env bash


mkdir -p target/llvm-cov/lcov

lcov_dir=target/llvm-cov/lcov
html_dir=target/llvm-cov/html

echo "Generating machine readable test report"
cargo llvm-cov --lcov --output-path $lcov_dir/lcov.info --quiet  && # machine readable format
echo "Generating human radable test report" &&
cargo llvm-cov --html --output-dir $html_dir --quiet                # human readable format


echo "The machine readable LCOV report is stored at $lcov_dir. The human readable report is stored at $html_dir/index.html."
