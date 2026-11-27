-- SPDX-License-Identifier: MIT OR Apache-2.0
ALTER TABLE contacts ADD COLUMN starred INTEGER NOT NULL DEFAULT 0;
