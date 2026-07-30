-- Builtin: deep-research
-- Ordinary registry definition using only public Lua host APIs.
-- Heterogeneous research children (distinct roles/domains) + structured final report.

local args = neo.args
local question = args.question
if type(question) ~= "string" or question == "" then
  neo.fail("deep-research requires non-empty args.question")
end

local depth = args.depth
if type(depth) ~= "string" or depth == "" then
  depth = "standard"
end

local finding_schema = {
  type = "object",
  additionalProperties = false,
  required = { "findings" },
  properties = {
    findings = {
      type = "array",
      items = {
        type = "object",
        additionalProperties = false,
        required = { "claim", "source", "evidence" },
        properties = {
          claim = { type = "string" },
          source = { type = "string" },
          evidence = { type = "string" },
        },
      },
    },
    gaps = {
      type = "array",
      items = { type = "string" },
    },
  },
}

neo.phase("plan")
local plan = {
  question = question,
  depth = depth,
  domains = { "primary_sources", "counterpoints", "context" },
}
neo.report({
  kind = "research_plan",
  plan = plan,
})
neo.log("research plan committed")

neo.phase("research")
-- Heterogeneous children: independent domains with distinct roles and ceilings.
local primary = neo.delegate({
  title = "primary_sources",
  task = "Gather primary sources and evidence for: " .. question,
  role = "explorer",
  worktree = "shared",
  tool_allow = { "Read", "List", "Grep", "Find", "Glob" },
  output_schema = finding_schema,
})
if not primary.ok then
  neo.fail(primary.summary)
end

local counterpoints = neo.delegate({
  title = "counterpoints",
  task = "Find counterpoints and contradictions for: " .. question,
  role = "reviewer",
  worktree = "shared",
  tool_allow = { "Read", "List", "Grep", "Find", "Glob" },
  output_schema = finding_schema,
})
if not counterpoints.ok then
  neo.fail(counterpoints.summary)
end

local context_child = neo.delegate({
  title = "context",
  task = "Collect background context for: " .. question,
  role = "planner",
  worktree = "shared",
  tool_allow = { "Read", "List", "Grep", "Find", "Glob" },
  output_schema = finding_schema,
})
if not context_child.ok then
  neo.fail(context_child.summary)
end

-- Prefer structured child findings; valid empty findings remain empty.
local findings = {}

local function try_child_findings(outcome, domain)
  local details = outcome.details
  if type(details) ~= "table" then
    return
  end
  local structured = details.structured_output
  local list = type(structured) == "table" and structured.findings or nil
  if type(list) ~= "table" then
    return
  end
  for i = 1, 32 do
    local finding = list[i]
    if finding == nil then
      break
    end
    findings[#findings + 1] = {
      claim = tostring(finding.claim or ""),
      source = tostring(finding.source or domain),
      evidence = tostring(finding.evidence or ""),
    }
  end
end

try_child_findings(primary, "primary_sources")
try_child_findings(counterpoints, "counterpoints")
try_child_findings(context_child, "context")

neo.phase("verify")
local verification = neo.delegate({
  title = "gap_check",
  task = "Cross-check research findings for contradictions and gaps on: " .. question,
  role = "reviewer",
  worktree = "shared",
  tool_allow = { "Read", "List", "Grep", "Find", "Glob" },
  output_schema = {
    type = "object",
    additionalProperties = false,
    required = { "ok", "contradictions", "gaps" },
    properties = {
      ok = { type = "boolean" },
      contradictions = { type = "array", items = { type = "string" } },
      gaps = { type = "array", items = { type = "string" } },
    },
  },
})
if not verification.ok then
  neo.fail(verification.summary)
end
local verification_output = type(verification.details) == "table"
  and verification.details.structured_output
if type(verification_output) ~= "table" or verification_output.ok ~= true then
  neo.fail("verification child reported ok=false")
end

if args.clarify == true then
  local answer = neo.await_user({
    prompt = "Clarify research direction before synthesis?",
    answer_schema = {
      type = "object",
      additionalProperties = false,
      required = { "continue" },
      properties = {
        continue = { type = "boolean" },
        notes = { type = "string" },
      },
    },
    answer_policy = "human",
  })
  if answer.continue ~= true then
    neo.fail("user declined to continue research")
  end
end

neo.phase("synthesize")
local report_text = "Synthesized research report for: " .. question
neo.report({
  kind = "final_research_report",
  question = question,
  findings_count = #findings,
  report = report_text,
})

findings = neo.json_array(findings)
return {
  ok = true,
  question = question,
  depth = depth,
  plan = plan,
  findings = findings,
  report = report_text,
  confidence = 0.75,
  artifacts = { "research_plan", "final_research_report" },
}
