// Stub: only module-level usage must not throw; wormhole (SMB2) paths are never exercised.
const mkNs = (): any => new Proxy({}, { get: (_t, k) => (k === 'create' || k === 'fromValues' || k === 'clone' || k === 'zero' || k === 'set' ? () => new Float32Array(16) : () => { throw new Error("gl-matrix stub: " + String(k)); }) });
export const vec3 = mkNs(); export const mat4 = mkNs(); export const mat3 = mkNs(); export const quat = mkNs();
export const vec2 = mkNs(); export const vec4 = mkNs();
