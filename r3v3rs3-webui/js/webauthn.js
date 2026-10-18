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

export async function createPasskey(challengeB64, rpId, rpName, userIdStr, userName, userDisplayName) {
    if (!isWebAuthnSupported()) {
        throw new Error("WebAuthn is not supported in this browser");
    }

    const challenge = base64ToBuffer(challengeB64);
    const enc = new TextEncoder();
    const userId = enc.encode(userIdStr || userName || "user");

    const publicKeyCredentialCreationOptions = {
        challenge,
        rp: {
            name: rpName || "r3v3rs3",
            id: rpId || undefined
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

    const credential = await navigator.credentials.create({
        publicKey: publicKeyCredentialCreationOptions
    });

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

    const challenge = base64ToBuffer(challengeB64);

    const publicKeyCredentialRequestOptions = {
        challenge,
        rpId: rpId || undefined,
        userVerification: "preferred",
        timeout: 60000
    };

    const assertion = await navigator.credentials.get({
        publicKey: publicKeyCredentialRequestOptions
    });

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
