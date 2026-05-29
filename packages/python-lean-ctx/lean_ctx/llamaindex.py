"""LlamaIndex integration for lean-ctx."""

from typing import Optional

from lean_ctx.client import LeanCtxClient

try:
    from llama_index.core.node_parser import NodeParser
    from llama_index.core.schema import BaseNode, TextNode, Document

    class LeanCtxNodeParser(NodeParser):
        """LlamaIndex node parser using lean-ctx compression modes."""

        client: LeanCtxClient = None
        mode: str = "map"

        def __init__(self, project_root: Optional[str] = None, mode: str = "map", **kwargs):
            super().__init__(**kwargs)
            self.client = LeanCtxClient(project_root=project_root)
            self.mode = mode

        def _parse_nodes(self, nodes: list[BaseNode], **kwargs) -> list[BaseNode]:
            result_nodes = []
            for node in nodes:
                if isinstance(node, Document) and hasattr(node, "metadata"):
                    file_path = node.metadata.get("file_path", "")
                    if file_path:
                        compressed = self.client.read(file_path, mode=self.mode)
                        result_nodes.append(
                            TextNode(
                                text=compressed,
                                metadata={**node.metadata, "compression_mode": self.mode},
                            )
                        )
                        continue
                result_nodes.append(node)
            return result_nodes

except ImportError:

    class LeanCtxNodeParser:
        """Stub: install llama-index-core for full integration."""

        def __init__(self, **kwargs):
            raise ImportError(
                "llama-index-core is required: pip install llama-index-core"
            )
