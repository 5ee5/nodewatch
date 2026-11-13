"use strict";

(() => {
  const form = document.getElementById("login-form");
  const input = document.getElementById("token");
  const button = document.getElementById("submit");
  const error = document.getElementById("error");

  function fail(message) {
    error.textContent = message;
    button.disabled = false;
    input.focus();
    input.select();
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    error.textContent = "";
    button.disabled = true;

    try {
      const res = await fetch("/api/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token: input.value.trim() }),
      });
      if (res.ok) {
        // The token is never stored client-side; the server set an HttpOnly cookie.
        input.value = "";
        location.assign("/");
        return;
      }
      if (res.status === 401) return fail("Invalid access token.");
      if (res.status === 413) return fail("That token is too long.");
      fail("Sign-in failed (HTTP " + res.status + ").");
    } catch (_) {
      fail("Cannot reach the server.");
    }
  });
})();
