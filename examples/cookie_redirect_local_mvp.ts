import { runScriptScenario } from "../ts/mod.ts";

const execModeEnv = (Deno.env.get("EXEC_MODE") ?? "fetch").toLowerCase();
const executionMode = execModeEnv === "ffi-step" ? "ffi-step" : "fetch";

const controller = new AbortController();

const server = Deno.serve(
  { hostname: "127.0.0.1", port: 0, signal: controller.signal },
  (req) => {
    const url = new URL(req.url);

    if (url.pathname === "/set-cookie") {
      return new Response(null, {
        status: 302,
        headers: {
          location: "/check-cookie",
          "set-cookie": "sid=abc123; Path=/; HttpOnly",
        },
      });
    }

    if (url.pathname === "/check-cookie") {
      const cookie = req.headers.get("cookie") ?? "";
      if (!cookie.includes("sid=abc123")) {
        return new Response(JSON.stringify({ ok: false, cookie }), {
          status: 401,
          headers: { "content-type": "application/json" },
        });
      }

      return new Response(JSON.stringify({ ok: true, cookie }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }

    return new Response("not found", { status: 404 });
  },
);

const addr = server.addr as Deno.NetAddr;
const baseUrl = `http://${addr.hostname}:${addr.port}`;

try {
  const result = await runScriptScenario({
    vus: 1,
    duration_s: 1,
    request_timeout_s: 5,
    continue_on_error: false,
    use_cookies: true,
    redirect_policy: "follow",
    execution_mode: executionMode,
    step_session_config: executionMode === "ffi-step"
      ? {
        protocol: "h1",
        request_timeout_s: 5,
        cookie_jar: true,
        redirect_policy: "follow",
        response_headers: true,
      }
      : undefined,
    steps: [
      {
        name: "set_cookie_manual_redirect",
        method: "GET",
        url: `${baseUrl}/set-cookie`,
        expected_status: [302],
        redirect_policy: "manual",
        extract: [
          {
            type: "header",
            name: "location",
            as: "redirect_path",
          },
        ],
        checks: {
          header_exists: ["set-cookie", "location"],
        },
      },
      {
        name: "followup_cookie_check",
        method: "GET",
        url: `${baseUrl}{{redirect_path}}`,
        expected_status: 200,
        checks: {
          body_includes: ['"ok":true', "sid=abc123"],
          json_path_equals: {
            ok: true,
          },
        },
      },
    ],
  });

  console.log(JSON.stringify(result, null, 2));
} finally {
  controller.abort();
  await server.finished;
}
