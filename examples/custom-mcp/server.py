"""Minimal zero-dependency Python stdio MCP server template for Yokahu."""
import json
import sys


def handle_tool(name: str, args: dict) -> dict:
    if name == "echo":
        text = args.get("text", "")
        return {"content": [{"type": "text", "text": f"Echo: {text}"}]}
    return {"content": [{"type": "text", "text": f"Unknown tool: {name}"}], "isError": True}


TOOLS = [
    {
        "name": "echo",
        "description": "Example custom MCP tool that echoes input text.",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string", "description": "Text to echo"}},
            "required": ["text"],
        },
    }
]


def main() -> None:
    for line in sys.stdin:
        line = line.lstrip("\ufeff").strip()
        if not line:
            continue
        req = json.loads(line)
        method = req.get("method", "")
        if method == "notifications/initialized" or "id" not in req:
            continue
        req_id = req["id"]
        params = req.get("params") or {}

        if method == "initialize":
            res = {
                "protocolVersion": params.get("protocolVersion", "2024-11-05"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "custom-mcp", "version": "0.1.0"},
            }
        elif method == "ping":
            res = {}
        elif method == "tools/list":
            res = {"tools": TOOLS}
        elif method == "tools/call":
            res = handle_tool(params.get("name", ""), params.get("arguments") or {})
        else:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "error": {"code": -32601, "message": "Method not found"}}) + "\n")
            sys.stdout.flush()
            continue

        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": res}) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
