function bufferToBase64(buffer) {
    const bytes = new Uint8Array(buffer);
    let binary = '';
    for (let i = 0; i < bytes.byteLength; i++) {
        binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary);
}

function base64ToBuffer(base64) {
    let str = base64.replace(/-/g, '+').replace(/_/g, '/');
    while (str.length % 4) {
        str += '=';
    }
    const binary = atob(str);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
        bytes[i] = binary.charCodeAt(i);
    }
    return bytes.buffer;
}

export function isWebAuthnSupported() {
    return typeof window !== 'undefined' &&
        window.PublicKeyCredential !== undefined &&
        typeof window.PublicKeyCredential === 'function';
}

function isIpAddress(host) {
    if (!host) return false;
    return /^(\d{1,3}\.){3}\d{1,3}$/.test(host) || host.includes(':');
}

function resolveRpId(serverRpId) {
    if (typeof window === 'undefined' || !window.location) {
        return serverRpId || undefined;
    }
    const host = window.location.hostname;
    if (!host) {
        return serverRpId || undefined;
    }
    if (host === 'localhost' || host.endsWith('.localhost')) {
        return 'localhost';
    }
    if (serverRpId && (host === serverRpId || host.endsWith('.' + serverRpId))) {
        return serverRpId;
    }
    if (!isIpAddress(host)) {
        return host;
    }
    return undefined;
}

export async function createPasskey(challengeB64, rpId, rpName, userIdStr, userName, userDisplayName) {
    if (!isWebAuthnSupported()) {
        throw new Error("WebAuthn is not supported in this browser");
    }

    const host = (typeof window !== 'undefined' && window.location) ? window.location.hostname : '';
    if (isIpAddress(host)) {
        if (host === '127.0.0.1' || host === '::1') {
            const port = (window.location && window.location.port) ? (':' + window.location.port) : '';
            throw new Error("PASSKEY_ORIGIN_IP: WebAuthn passkeys cannot be created on IP address 127.0.0.1. Please open the panel using http://localhost" + port + " instead.");
        } else {
            throw new Error("PASSKEY_ORIGIN_IP_OTHER: WebAuthn passkeys require a domain name or 'localhost' (browsers do not support passkeys on IP addresses such as " + host + "). Please access via a domain name.");
        }
    }

    const effectiveRpId = resolveRpId(rpId);

    const challenge = base64ToBuffer(challengeB64);
    const enc = new TextEncoder();
    const userId = enc.encode(userIdStr || userName || "user");

    const publicKeyCredentialCreationOptions = {
        challenge,
        rp: {
            name: rpName || "r3v3rs3",
            id: effectiveRpId
        },
        user: {
            id: userId,
            name: userName || "admin",
            displayName: userDisplayName || userName || "Admin"
        },
        pubKeyCredParams: [
            { alg: -7, type: "public-key" },   // ES256 (P-256)
            { alg: -257, type: "public-key" }  // RS256
        ],
        authenticatorSelection: {
            residentKey: "preferred",
            requireResidentKey: false,
            userVerification: "preferred"
        },
        timeout: 60000,
        attestation: "none"
    };

    let credential;
    try {
        credential = await navigator.credentials.create({
            publicKey: publicKeyCredentialCreationOptions
        });
    } catch (err) {
        if (err && err.message && (err.message.includes("cannot be used with the current origin") || err.message.includes("relying party ID"))) {
            throw new Error("PASSKEY_ORIGIN_MISMATCH: " + err.message);
        }
        throw err;
    }

    if (!credential) {
        throw new Error("Passkey creation was cancelled");
    }

    let publicKeyB64 = "";
    if (typeof credential.response.getPublicKey === 'function') {
        const pkBuffer = credential.response.getPublicKey();
        if (pkBuffer) {
            publicKeyB64 = bufferToBase64(pkBuffer);
        }
    }

    const credentialId = bufferToBase64(credential.rawId);
    const clientDataJson = bufferToBase64(credential.response.clientDataJSON);

    return JSON.stringify({
        credential_id: credentialId,
        public_key: publicKeyB64,
        client_data_json: clientDataJson
    });
}

export async function getPasskey(challengeB64, rpId) {
    if (!isWebAuthnSupported()) {
        throw new Error("WebAuthn is not supported in this browser");
    }

    const effectiveRpId = resolveRpId(rpId);

    const challenge = base64ToBuffer(challengeB64);

    const publicKeyCredentialRequestOptions = {
        challenge,
        rpId: effectiveRpId,
        userVerification: "preferred",
        timeout: 60000
    };

    let assertion;
    try {
        assertion = await navigator.credentials.get({
            publicKey: publicKeyCredentialRequestOptions
        });
    } catch (err) {
        if (err && err.message && (err.message.includes("cannot be used with the current origin") || err.message.includes("relying party ID"))) {
            throw new Error("PASSKEY_ORIGIN_MISMATCH: " + err.message);
        }
        throw err;
    }

    if (!assertion) {
        throw new Error("Passkey assertion was cancelled");
    }

    const credentialId = bufferToBase64(assertion.rawId);
    const authenticatorData = bufferToBase64(assertion.response.authenticatorData);
    const clientDataJson = bufferToBase64(assertion.response.clientDataJSON);
    const signature = bufferToBase64(assertion.response.signature);

    return JSON.stringify({
        credential_id: credentialId,
        authenticator_data: authenticatorData,
        client_data_json: clientDataJson,
        signature: signature
    });
}
