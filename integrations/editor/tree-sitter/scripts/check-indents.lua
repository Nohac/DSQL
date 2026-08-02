local package_root = assert(os.getenv("DSQL_TREE_SITTER_ROOT"), "missing DSQL_TREE_SITTER_ROOT")
local parser_path = assert(os.getenv("DSQL_TREE_SITTER_PARSER"), "missing DSQL_TREE_SITTER_PARSER")
local nvim_treesitter_path = os.getenv("DSQL_NVIM_TREESITTER_PATH")

if nvim_treesitter_path and nvim_treesitter_path ~= "" then
  vim.opt.runtimepath:prepend(nvim_treesitter_path)
else
  local loaded, error_message = pcall(vim.cmd, "packadd nvim-treesitter")
  if not loaded then
    error("cannot load nvim-treesitter: " .. tostring(error_message))
  end
end

local loaded, indent = pcall(require, "nvim-treesitter.indent")
if not loaded then
  error("cannot load nvim-treesitter indentation engine: " .. tostring(indent))
end

vim.treesitter.language.add("dsql", { path = parser_path })
local query_lines = vim.fn.readfile(package_root .. "/queries/indents.scm")
vim.treesitter.query.set("dsql", "indents", table.concat(query_lines, "\n"))

local cases = {
  {
    name = "definition blocks and headers",
    source = [[filter Search on {
  .title: text
} {
  field title where .title like $title
}

condition Visible on title {
  where .id is not null
}

fragment Bits(
  $kind? = null
) on title {
  # fields stay aligned after a comment
  id
}]],
    expected = {
      { 2, 2 },
      { 3, 0 },
      { 4, 2 },
      { 5, 0 },
      { 8, 2 },
      { 9, 0 },
      { 12, 2 },
      { 13, 0 },
      { 14, 2 },
      { 15, 2 },
      { 16, 0 },
    },
  },
  {
    name = "nested lists, expressions, directives, and selections",
    source = [[query Find(
  %ids? = [
    1,
    2,
  ]
  %comparison? = "=="
) {
  title(
    where (
      .id %comparison[
        ==,
        !=,
      ] 1
      and exists .cast_info(
        where .note != null
      )
      and .id in [
        1,
        2,
      ]
    )
    order by title asc
  ) @dsql.deprecated(
    reason: "old"
  ) {
    ...Bits(
      $kind <- %kind,
    )
    ratings | aggregate {
      count
    }
  }
}]],
    expected = {
      { 2, 2 },
      { 3, 4 },
      { 5, 2 },
      { 7, 0 },
      { 8, 2 },
      { 9, 4 },
      { 10, 6 },
      { 11, 8 },
      { 13, 6 },
      { 14, 6 },
      { 15, 8 },
      { 16, 6 },
      { 17, 6 },
      { 18, 8 },
      { 20, 6 },
      { 21, 4 },
      { 23, 2 },
      { 24, 4 },
      { 25, 2 },
      { 26, 4 },
      { 27, 6 },
      { 28, 4 },
      { 29, 4 },
      { 30, 6 },
      { 31, 4 },
      { 32, 2 },
      { 33, 0 },
    },
  },
  {
    name = "blank lines and split empty objects follow their parent",
    source = [[query BlankLines(
  %ids? = [
    1,
  ]

  %value? = {
  }
) {
  title(
    where .id in %ids
  )

  nested {
    id
  }

  next
}

query After {
  id
}]],
    expected = {
      { 4, 2 },
      { 5, 2 },
      { 7, 2 },
      { 8, 0 },
      { 9, 2 },
      { 11, 2 },
      { 12, 2 },
      { 13, 2 },
      { 14, 4 },
      { 15, 2 },
      { 16, 2 },
      { 17, 2 },
      { 18, 0 },
      { 19, 0 },
      { 21, 2 },
      { 22, 0 },
    },
  },
  {
    name = "order and aggregate continuations",
    source = [[query Continuations {
  title(
    order by
      %sort on selected,
      %indexed_sort on indexed
    limit 10
  ) {
    ratings | stats by
      year: .production_year,
      kind: .kind {
        count
      }
  }
}]],
    expected = {
      { 2, 2 },
      { 3, 4 },
      { 4, 6 },
      { 5, 6 },
      { 6, 4 },
      { 7, 2 },
      { 8, 4 },
      { 9, 6 },
      { 10, 6 },
      { 11, 8 },
      { 12, 6 },
      { 13, 2 },
      { 14, 0 },
    },
  },
  {
    name = "single unfinished container",
    source = [[query Broken(
  %id?]],
    expected = {
      { 2, 2 },
    },
  },
  {
    name = "multiple unfinished containers recover one level",
    source = [[query Broken {
  title(
  where .id =]],
    expected = {
      { 2, 2 },
      { 3, 2 },
    },
  },
  {
    name = "unrelated error does not disturb a valid block",
    source = [[query Good {
  title {
    id
  }
}

query Broken (]],
    expected = {
      { 2, 2 },
      { 3, 4 },
      { 4, 2 },
      { 5, 0 },
    },
  },
}

for _, case in ipairs(cases) do
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_win_set_buf(0, buffer)
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, vim.split(case.source, "\n", { plain = true }))
  vim.bo[buffer].shiftwidth = 2
  vim.bo[buffer].tabstop = 2
  vim.bo[buffer].expandtab = true
  vim.treesitter.start(buffer, "dsql")
  vim.treesitter.get_parser(buffer, "dsql"):parse()

  for _, expectation in ipairs(case.expected) do
    local line = expectation[1]
    local expected = expectation[2]
    local actual = indent.get_indent(line)
    if actual ~= expected then
      error(string.format(
        "%s line %d: expected indent %d, got %d\n%s",
        case.name,
        line,
        expected,
        actual,
        case.source
      ))
    end
  end

  vim.treesitter.stop(buffer)
  vim.api.nvim_buf_delete(buffer, { force = true })
end
