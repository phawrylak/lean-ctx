#!/bin/sh
# Unit test for the factorial code-edit task (#730). Exit 0 == pass.
. ./solution.sh
[ "$(factorial 0)" = "1" ] || exit 1
[ "$(factorial 1)" = "1" ] || exit 1
[ "$(factorial 5)" = "120" ] || exit 1
[ "$(factorial 6)" = "720" ] || exit 1
