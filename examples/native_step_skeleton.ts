import { LoadgenStepFFI } from "../ts/mod.ts";

const stepSession = new LoadgenStepFFI({
  protocol: "h2",
  request_timeout_s: 10,
  cookie_jar: true,
  redirect_policy: "follow",
  response_body_limit: 65536,
  response_headers: true,
});

try {
  console.log(`step_abi_version=${stepSession.abiVersion()}`);

  const stepResponse = await stepSession.execute({
    name: "login",
    method: "POST",
    url: "https://quickpizza.grafana.com/api/users/token/login",
    headers: {
      "content-type": "application/json",
    },
    body: "{\"username\":\"default\",\"password\":\"12345678\"}",
    capture_body: true,
  });

  console.log("step_execute_response:");
  console.log(JSON.stringify(stepResponse, null, 2));

  console.log("step_snapshot:");
  console.log(JSON.stringify(stepSession.snapshot(), null, 2));
} finally {
  stepSession.close();
}
