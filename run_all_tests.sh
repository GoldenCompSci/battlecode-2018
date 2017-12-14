#!/bin/bash

# Runs all the test by hand, in a shell.
# TODO: windows.

pushd $(dirname $0) >/dev/null

echo
echo
echo -e "--\033[32m Running rust tests \033[0m--\n"
echo
echo
echo -e '\033[33m$ cargo test \033[0m'
cargo test
RUST=$?
echo

battlecode-engine-c/run_tests.sh
C=$?
echo

battlecode-engine-py/run_tests.sh
PYTHON=$?
echo
echo
echo '---------------'

if [ $RUST -ne 0 ]; then
    echo -e '\033[31mRust failed!\033[0m'
fi
if [ $C -ne 0 ]; then
    echo -e '\033[31mC failed!\033[0m'
fi
if [ $PYTHON -ne 0 ]; then
    echo -e '\033[31mPython failed!\033[0m'
fi

if [ $RUST -ne 0 -o $C -ne 0 -o $PYTHON -ne 0 ]; then
    echo -e '\033[31mTests failed!\033[0m'
    exit 1
else
    echo
    echo 'All tests passed.'
fi

popd >/dev/null
