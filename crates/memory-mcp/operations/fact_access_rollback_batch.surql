-- Offline operator transaction. Bind fact_access_rollback_keys to the next
-- batch of exact side-record keys while all application writers are stopped.
BEGIN TRANSACTION;

IF (SELECT VALUE status FROM script_migration:057_fact_access_isolation_surql) != ['applied'] {
    THROW 'fact access rollback requires a completed migration';
};
IF array::len($fact_access_rollback_keys) = 0 OR array::len($fact_access_rollback_keys) > 128 {
    THROW 'fact access rollback requires between 1 and 128 keys';
};

FOR $key IN $fact_access_rollback_keys {
    LET $side = SELECT source_episode, access_count, last_accessed
        FROM type::record('fact_access', $key);
    LET $fact = type::record('fact', $key);
    LET $source = SELECT fact_id, source_episode FROM $fact;
    IF array::len($side) != 1 OR array::len($source) != 1
        OR $source[0].fact_id != string::concat('fact:', $key)
        OR $source[0].source_episode != $side[0].source_episode {
        THROW 'fact access rollback found a missing or inconsistent fact identity';
    };

    UPDATE $fact SET
        access_count = $side[0].access_count,
        last_accessed = $side[0].last_accessed
        RETURN NONE;

    LET $copied = SELECT access_count, last_accessed FROM $fact;
    IF array::len($copied) != 1 OR $copied[0].access_count != $side[0].access_count
        OR $copied[0].last_accessed != $side[0].last_accessed {
        THROW 'fact access rollback did not copy both heat fields exactly';
    };
};

COMMIT TRANSACTION;
