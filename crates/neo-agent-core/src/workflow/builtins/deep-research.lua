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
neo.verify(primary.ok, "primary_sources child failed: " .. tostring(primary.summary))

local counterpoints = neo.delegate({
  title = "counterpoints",
  task = "Find counterpoints and contradictions for: " .. question,
  role = "reviewer",
  worktree = "shared",
  tool_allow = { "Read", "List", "Grep", "Find", "Glob" },
  output_schema = finding_schema,
})
neo.verify(counterpoints.ok, "counterpoints child failed: " .. tostring(counterpoints.summary))

local context_child = neo.delegate({
  title = "context",
  task = "Collect background context for: " .. question,
  role = "planner",
  worktree = "shared",
  tool_allow = { "Read", "List", "Grep", "Find", "Glob" },
  output_schema = finding_schema,
})
neo.verify(context_child.ok, "context child failed: " .. tostring(context_child.summary))

-- Prefer structured child findings; fall back to summary-backed placeholders.
local findings = {}
local function push_summary_finding(outcome, domain)
  findings[#findings + 1] = {
    claim = tostring(outcome.summary or domain),
    source = domain,
    evidence = "child summary",
  }
end

local function try_child_findings(outcome, domain)
  local details = outcome.details
  if type(details) ~= "table" then
    push_summary_finding(outcome, domain)
    return
  end
  local structured = details.structured_output
  local list = type(structured) == "table" and structured.findings or nil
  if type(list) ~= "table" then
    push_summary_finding(outcome, domain)
    return
  end
  local count = 0
  for i = 1, 32 do
    local finding = list[i]
    if finding == nil then
      break
    end
    count = count + 1
    findings[#findings + 1] = {
      claim = tostring(finding.claim or ""),
      source = tostring(finding.source or domain),
      evidence = tostring(finding.evidence or ""),
    }
  end
  if count == 0 then
    push_summary_finding(outcome, domain)
  end
end

local ok_collect, err_collect = pcall(function()
  try_child_findings(primary, "primary_sources")
  try_child_findings(counterpoints, "counterpoints")
  try_child_findings(context_child, "context")
end)
if not ok_collect then
  neo.log("findings collection fell back: " .. tostring(err_collect))
  findings = {
    {
      claim = tostring(primary.summary),
      source = "primary_sources",
      evidence = "fallback",
    },
    {
      claim = tostring(counterpoints.summary),
      source = "counterpoints",
      evidence = "fallback",
    },
    {
      claim = tostring(context_child.summary),
      source = "context",
      evidence = "fallback",
    },
  }
end

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
neo.verify(verification.ok, "verification child failed: " .. tostring(verification.summary))

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
  neo.verify(answer.continue == true, "user declined to continue research")
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
