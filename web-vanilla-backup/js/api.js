/** Thin fetch wrappers over pecan's JSON API. */

/** GET a JSON endpoint, rejecting with the server's error message. */
export async function get(path) {
  const res = await fetch(path);
  if (!res.ok) throw new Error(await errorMessage(res));
  return res.json();
}

/** POST a JSON body. */
export async function post(path, body = {}) {
  const res = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(await errorMessage(res));
  return res.json();
}

/** DELETE with an optional JSON body. */
export async function del(path, body = null) {
  const res = await fetch(path, {
    method: "DELETE",
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : null,
  });
  if (!res.ok) throw new Error(await errorMessage(res));
  return res.json();
}

async function errorMessage(res) {
  try {
    const data = await res.json();
    return data.error ?? res.statusText;
  } catch {
    return res.statusText;
  }
}
