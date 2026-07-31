-- Builtin: large-refactor
-- Mutation-capable slices default to isolated worktrees.
-- Merge/retirement is always an explicit human decision — never auto-merge or delete.

local args = neo.args
local spec = args.spec
if type(spec) ~= "string" or spec == "" then
  neo.fail("large-refactor requires non-empty args.spec")
end

local plan = args.plan
if type(plan) ~= "string" or plan == "" then
  plan = "partition independent slices from approved spec"
end

local slice_schema = {
  type = "object",
  additionalProperties = false,
  required = { "ok", "slice_id", "summary", "commits", "verification" },
  properties = {
    ok = { type = "boolean" },
    slice_id = { type = "string" },
    summary = { type = "string" },
    commits = { type = "array", items = { type = "string" } },
    verification = { type = "string" },
    risks = { type = "array", items = { type = "string" } },
  },
}

neo.phase("intake")
neo.report({
  kind = "refactor_intake",
  spec = spec,
  plan = plan,
  worktree_policy = "isolated",
  auto_merge = false,
})
neo.log("approved refactor inputs captured")

neo.phase("partition")
-- Heterogeneous implementation slices; mutation children use isolated worktrees.
local slice_a = neo.delegate({
  title = "slice_a",
  task = "Implement refactor slice A for: " .. spec .. " following plan: " .. plan,
  role = "coder",
  worktree = "isolated",
  output_schema = slice_schema,
})
if not slice_a.ok then
  neo.fail(slice_a.summary)
end
local slice_a_output = type(slice_a.details) == "table" and slice_a.details.structured_output or nil
if type(slice_a_output) ~= "table" or slice_a_output.ok ~= true then
  neo.fail("slice_a reported ok=false")
end

local slice_b = neo.delegate({
  title = "slice_b",
  task = "Implement refactor slice B for: " .. spec .. " following plan: " .. plan,
  role = "coder",
  worktree = "isolated",
  output_schema = slice_schema,
})
if not slice_b.ok then
  neo.fail(slice_b.summary)
end
local slice_b_output = type(slice_b.details) == "table" and slice_b.details.structured_output or nil
if type(slice_b_output) ~= "table" or slice_b_output.ok ~= true then
  neo.fail("slice_b reported ok=false")
end

local review = neo.delegate({
  title = "slice_review",
  task = "Review isolated-slice results for: " .. spec,
  role = "reviewer",
  worktree = "shared",
  tool_allow = { "Read", "List", "Grep", "Find", "Glob" },
  output_schema = {
    type = "object",
    additionalProperties = false,
    required = { "ok", "summary", "risks" },
    properties = {
      ok = { type = "boolean" },
      summary = { type = "string" },
      risks = { type = "array", items = { type = "string" } },
    },
  },
})
if not review.ok then
  neo.fail(review.summary)
end
local review_output = type(review.details) == "table" and review.details.structured_output or nil
if type(review_output) ~= "table" or review_output.ok ~= true then
  neo.fail("slice review reported ok=false")
end

neo.report({
  kind = "slice_results",
  slices = {
    tostring(slice_a.summary),
    tostring(slice_b.summary),
    tostring(review.summary),
  },
})

neo.phase("merge_gate")
-- Explicit human merge/retirement decision. Never auto-merge or delete worktrees.
local decision = neo.await_user({
  prompt = "Approve merge of isolated refactor worktrees? (never auto-merge/delete)",
  answer_schema = {
    type = "object",
    additionalProperties = false,
    required = { "merge", "retire_worktrees" },
    properties = {
      merge = { type = "boolean" },
      retire_worktrees = { type = "boolean" },
      notes = { type = "string" },
    },
  },
  answer_policy = "human",
})

neo.verify(type(decision.merge) == "boolean", "merge decision must be boolean")
neo.verify(
  type(decision.retire_worktrees) == "boolean",
  "retire_worktrees decision must be boolean"
)

if decision.retire_worktrees == true and decision.merge ~= true then
  neo.log("retire requested without merge — worktrees left for manual review")
end

local unresolved = {}
if decision.merge ~= true then
  unresolved[#unresolved + 1] = "merge not approved; isolated worktrees retained"
end
if decision.retire_worktrees ~= true then
  unresolved[#unresolved + 1] = "worktree retirement not approved; no auto-delete"
end

if type(review_output.risks) == "table" then
  for i = 1, 32 do
    local risk = review_output.risks[i]
    if risk == nil then
      break
    end
    unresolved[#unresolved + 1] = tostring(risk)
  end
end

neo.phase("report")
local lineage = {
  slices = { "slice_a", "slice_b" },
  worktree_policy = "isolated",
  auto_merge = false,
  auto_delete_worktrees = false,
  merge_approved = decision.merge,
  retire_approved = decision.retire_worktrees,
}

neo.report({
  kind = "refactor_final",
  lineage = lineage,
  decision = {
    merge = decision.merge,
    retire_worktrees = decision.retire_worktrees,
  },
})

unresolved = neo.json_array(unresolved)
return {
  ok = true,
  spec = spec,
  plan = plan,
  lineage = lineage,
  merge = decision.merge,
  retire_worktrees = decision.retire_worktrees,
  verification = tostring(review.summary),
  unresolved_risks = unresolved,
  commits = neo.json_array({}),
}
