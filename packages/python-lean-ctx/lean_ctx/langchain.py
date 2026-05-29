"""LangChain integration for lean-ctx."""

from typing import Optional

from lean_ctx.client import LeanCtxClient

try:
    from langchain_core.retrievers import BaseRetriever
    from langchain_core.documents import Document
    from langchain_core.callbacks import CallbackManagerForRetrieverRun

    class LeanCtxRetriever(BaseRetriever):
        """LangChain retriever backed by lean-ctx hybrid search."""

        client: LeanCtxClient = None
        top_k: int = 10

        def __init__(self, project_root: Optional[str] = None, top_k: int = 10, **kwargs):
            super().__init__(**kwargs)
            self.client = LeanCtxClient(project_root=project_root)
            self.top_k = top_k

        def _get_relevant_documents(
            self, query: str, *, run_manager: CallbackManagerForRetrieverRun
        ) -> list[Document]:
            result = self.client.search(query)
            documents = []
            for line in result.split("\n"):
                if not line.strip():
                    continue
                parts = line.split(":", 2)
                if len(parts) >= 3:
                    file_path, line_num, content = parts[0], parts[1], parts[2]
                    documents.append(
                        Document(
                            page_content=content.strip(),
                            metadata={"source": file_path, "line": line_num},
                        )
                    )
                else:
                    documents.append(Document(page_content=line.strip()))

            return documents[: self.top_k]

except ImportError:

    class LeanCtxRetriever:
        """Stub: install langchain-core for full integration."""

        def __init__(self, **kwargs):
            raise ImportError(
                "langchain-core is required: pip install langchain-core"
            )
