# Copyright 2026 Monika
# Distributed under the terms of the GNU Affero General Public License v3

EAPI=8

inherit acct-user

DESCRIPTION="User for the Breeze Core service"

# See acct-group/breeze for why this is not a fixed number.
ACCT_USER_ID=-1
ACCT_USER_GROUPS=( breeze )

# The state directory doubles as the home directory, which is what every other
# packager here does too: the account owns /etc/breeze-core and needs nothing
# else anywhere on the filesystem.
ACCT_USER_HOME=/etc/breeze-core
ACCT_USER_SHELL=/sbin/nologin

# 0750, matching app-misc/breeze-core-bin's fperms on the same directory. The
# eclass defaults this to 0755 and installs the home directory itself, so
# leaving it out means two packages ship the same path with different modes and
# whichever merges last wins. The directory holds config.json, devices.json and
# the units' V3 credentials; it is not world-readable.
ACCT_USER_HOME_PERMS=0750

# Not optional, and not automatic: the eclass turns ACCT_USER_GROUPS into the
# RDEPEND on acct-group/breeze, but only when this is called explicitly here in
# global scope. Leaving it out is accepted by `egencache` and by
# `emerge --pretend`, and then fails during the real merge with
#   Ebuild error: acct-user_add_deps must have been called in global scope!
# which names the function rather than the omission.
acct-user_add_deps
