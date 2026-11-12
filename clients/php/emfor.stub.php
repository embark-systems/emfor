<?php

/** @generate-class-entries */

// Reference for emfor_arginfo.h, which is written by hand to match.

namespace Emfor {
    /**
     * Address of one backend of service $name: "ip:port", or "[ip]:port"
     * for IPv6. $affinity_key matters only for sticky services; null or ""
     * is no key.
     *
     * @throws LookupException on failure (errorCode is the shared code name)
     * @throws \ValueError when an argument contains a NUL byte or is not UTF-8
     */
    function lookup(string $name, ?string $affinity_key = null): string {}

    class LookupException extends \RuntimeException
    {
        /** UNAVAILABLE, INCOMPATIBLE, NOT_FOUND or ALL_DOWN. */
        public readonly string $errorCode;
    }
}
