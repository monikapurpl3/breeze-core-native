# Copyright 2026 Monika
# Distributed under the terms of the GNU Affero General Public License v3

EAPI=8

inherit acct-group

DESCRIPTION="Group for the Breeze Core service"

# -1 means "let Gentoo allocate one". A fixed GID is only correct for packages
# in the official tree, where the number is reserved in Gentoo's own registry;
# an overlay claiming a number would eventually collide with whatever the
# distribution later assigns it.
ACCT_GROUP_ID=-1
