// sdk-ts/src/pm-helpers.js -- store factory, header helpers, dynamic vars for sandbox:pm
// Must be 7-bit ASCII only (no Unicode, no special characters outside ASCII 32-126).

function makeStore(store) {
    return {
        get: function (key) {
            return Object.prototype.hasOwnProperty.call(store, key) ? store[key] : undefined;
        },
        set: function (key, val) {
            store[key] = val;
        },
        has: function (key) {
            return Object.prototype.hasOwnProperty.call(store, key);
        },
        unset: function (key) {
            delete store[key];
        },
        clear: function () {
            let keys = Object.keys(store);
            for (let i = 0; i < keys.length; i++) delete store[keys[i]];
        },
    };
}

function headerGet(headers, name) {
    if (!Array.isArray(headers)) return null;
    let lower = String(name).toLowerCase();
    for (let i = 0; i < headers.length; i++) {
        if (Array.isArray(headers[i]) && String(headers[i][0]).toLowerCase() === lower) {
            return headers[i][1];
        }
    }
    return null;
}

function headerHas(headers, name) {
    if (!Array.isArray(headers)) return false;
    let lower = String(name).toLowerCase();
    for (let i = 0; i < headers.length; i++) {
        if (Array.isArray(headers[i]) && String(headers[i][0]).toLowerCase() === lower) {
            return true;
        }
    }
    return false;
}

function uuid4() {
    return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function (c) {
        let r = (Math.random() * 16) | 0;
        return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
    });
}

function resolveDynVar(name) {
    switch (name) {
        case "$guid":
        case "$randomUUID":
            return uuid4();
        case "$timestamp":
            return String(Math.floor(Date.now() / 1000));
        case "$isoTimestamp":
            return new Date().toISOString();
        case "$randomInt":
            return String(Math.floor(Math.random() * 1001));
        case "$randomFloat":
            return Math.random().toFixed(6);
        case "$randomBoolean":
            return String(Math.random() < 0.5);
        default:
            return null;
    }
}

// replaceIn resolves {{$dynamic}} and {{varName}} placeholders in a string.
// vars must be passed in; pm.js closes over _vars to create a bound version.
function replaceIn(str, vars) {
    return String(str).replace(/\{\{([^}]+)}}/g, function (match, name) {
        let key = name.trim();
        if (key.charAt(0) === "$") {
            let dyn = resolveDynVar(key);
            if (dyn !== null) return dyn;
        }
        return Object.prototype.hasOwnProperty.call(vars, key) ? String(vars[key]) : match;
    });
}

export {makeStore, headerGet, headerHas, replaceIn};
