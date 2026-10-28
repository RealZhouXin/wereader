-- Migration runner wraps this in a transaction. Membership retains server IDs,
-- including IDs not in the current electronic-book shelf (e.g. other media).
CREATE TABLE shelf_groups (
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    group_id TEXT NOT NULL CHECK (length(group_id) > 0),
    name TEXT NOT NULL,
    sort_order INTEGER NOT NULL,
    book_ids_json TEXT NOT NULL,
    PRIMARY KEY (account_id, group_id)
);
PRAGMA user_version = 3;
