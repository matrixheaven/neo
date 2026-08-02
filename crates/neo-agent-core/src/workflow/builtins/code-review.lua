-- Builtin: code-review
-- Read-only review domains with structured findings-first final output.
-- Never modifies code (mutation tools are excluded from child tool_allow).

local args = neo.args
local scope = args.scope
if type(scope) ~= "string" or scope == "" then
  neo.fail("code-review requires non-empty args.scope")
end

local criteria = args.criteria
if type(criteria) ~= "string" or criteria == "" then
  criteria = "correctness, security, maintainability, test coverage"
end

-- Read-only capability ceiling: no Write/Edit/Bash/Terminal.
local READ_ONLY_TOOLS = { "Read", "List", "Grep", "Find", "Glob" }

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
        required = { "severity", "path", "line", "evidence", "test_gap" },
        properties = {
          severity = { type = "string" },
          path = { type = "string" },
          line = { type = "integer" },
          evidence = { type = "string" },
          test_gap = { type = "string" },
        },
      },
    },
  },
}

neo.phase("scope")
neo.report({
  kind = "review_scope",
  scope = scope,
  criteria = criteria,
  tool_allow = READ_ONLY_TOOLS,
})
neo.log("code review scope accepted (read-only children)")

neo.phase("review")
-- Heterogeneous independent review domains with read-only ceilings.
local security = neo.delegate({
  title = "security",
  task = "Security review of scope `" .. scope .. "` against: " .. criteria,
  role = "reviewer",
  worktree = "shared",
  tool_allow = READ_ONLY_TOOLS,
  output_schema = finding_schema,
})
if security.status ~= "completed" then
  neo.fail(security.summary)
end

local correctness = neo.delegate({
  title = "correctness",
  task = "Correctness review of scope `" .. scope .. "` against: " .. criteria,
  role = "reviewer",
  worktree = "shared",
  tool_allow = READ_ONLY_TOOLS,
  output_schema = finding_schema,
})
if correctness.status ~= "completed" then
  neo.fail(correctness.summary)
end

local maintainability = neo.delegate({
  title = "maintainability",
  task = "Maintainability and test-gap review of scope `" .. scope .. "`",
  role = "explorer",
  worktree = "shared",
  tool_allow = READ_ONLY_TOOLS,
  output_schema = finding_schema,
})
if maintainability.status ~= "completed" then
  neo.fail(maintainability.summary)
end

local findings = {}
local seen = {}
local projection_gaps = {}

local function append_findings(outcome, domain)
  local details = outcome.details
  if type(details) ~= "table" then
    projection_gaps[#projection_gaps + 1] = domain
    return
  end
  local structured = details.structured_output
  local list = type(structured) == "table" and structured.findings or nil
  if type(list) ~= "table" then
    projection_gaps[#projection_gaps + 1] = domain
    return
  end
  for i = 1, 64 do
    local finding = list[i]
    if finding == nil then
      break
    end
    local path = tostring(finding.path or "")
    local line = tonumber(finding.line) or 0
    local severity = tostring(finding.severity or "info")
    local key = path .. ":" .. tostring(line) .. ":" .. severity
    if not seen[key] then
      seen[key] = true
      findings[#findings + 1] = {
        severity = severity,
        path = path,
        line = line,
        evidence = tostring(finding.evidence or ""),
        test_gap = tostring(finding.test_gap or ""),
      }
    end
  end
end

append_findings(security, "security")
append_findings(correctness, "correctness")
append_findings(maintainability, "maintainability")

neo.phase("challenge")
local challenge = neo.delegate({
  title = "challenge_weak_findings",
  task = "Challenge weak findings and drop unsupported claims for scope `" .. scope .. "`",
  role = "reviewer",
  worktree = "shared",
  tool_allow = READ_ONLY_TOOLS,
  output_schema = finding_schema,
})
if challenge.status ~= "completed" then
  neo.fail(challenge.summary)
end
append_findings(challenge, "challenge")

-- A missing structured projection is business data: keep completed findings
-- and report a deterministic partial result instead of failing the Workflow.
local status = "verified"
local gap_notes = {}
if #projection_gaps > 0 then
  status = "partial"
  for i = 1, 32 do
    local domain = projection_gaps[i]
    if domain == nil then
      break
    end
    gap_notes[#gap_notes + 1] = "structured findings unavailable for review domain " .. domain
  end
end

neo.report({
  kind = "review_findings",
  scope = scope,
  findings_count = #findings,
  status = status,
  projection_gaps = neo.json_array(projection_gaps),
})

-- Findings-first final output (findings is the primary required field).
findings = neo.json_array(findings)
return {
  findings = findings,
  status = status,
  scope = scope,
  criteria = criteria,
  summary = "code review complete for " .. scope,
  read_only = true,
}
