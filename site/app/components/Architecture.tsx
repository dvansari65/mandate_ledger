"use client";

import { Handle, MarkerType, Position, ReactFlow, type Edge, type Node, type NodeProps } from "@xyflow/react";
import "@xyflow/react/dist/style.css";

type Card = Node<{ title: string; sub?: string; tone?: "ok" | "gate" | "ink" }, "card">;
type Frame = Node<{ title: string; sub: string }, "frame">;

function CardNode({ data }: NodeProps<Card>) {
  return (
    <div className={`rf-card ${data.tone ?? ""}`}>
      <Handle type="target" position={Position.Left} id="l" />
      <Handle type="target" position={Position.Top} id="t" />
      <span className="rf-title">{data.title}</span>
      {data.sub && <span className="rf-sub">{data.sub}</span>}
      <Handle type="source" position={Position.Right} id="r" />
      <Handle type="source" position={Position.Bottom} id="b" />
    </div>
  );
}

function FrameNode({ data }: NodeProps<Frame>) {
  return (
    <div className="rf-frame">
      <span className="rf-frame-title">{data.title}</span>
      <span className="rf-frame-sub">{data.sub}</span>
    </div>
  );
}

const nodeTypes = { card: CardNode, frame: FrameNode };

const card = (id: string, x: number, y: number, title: string, sub?: string, tone?: Card["data"]["tone"], parentId?: string): Card => ({
  id, type: "card", position: { x, y }, data: { title, sub, tone },
  ...(parentId ? { parentId, extent: "parent" as const } : {}),
});

const nodes: Node[] = [
  card("wallet", 0, 40, "Wallet", "signs the mandate", "ink"),
  card("agent", 0, 250, "Agent", "builds the cart", "ink"),
  { id: "ledger", type: "frame", position: { x: 300, y: -30 }, data: { title: "mandate-ledger", sub: "does not move money" }, style: { width: 600, height: 380 } } as Frame,
  card("sig", 30, 70, "signature", "signer trust", "gate", "ledger"),
  card("scope", 30, 140, "scope", "merchant · category · cap", "gate", "ledger"),
  card("budget", 30, 210, "budget", "velocity · atomic", "gate", "ledger"),
  card("authorized", 30, 300, "authorized", undefined, "ok", "ledger"),
  card("paid", 175, 300, "paid", undefined, "ok", "ledger"),
  card("settled", 320, 300, "settled", undefined, "ok", "ledger"),
  card("delivered", 465, 300, "delivered", undefined, "ok", "ledger"),
  card("rail", 520, 440, "Rail", "PSP · UPI · USDC on Base", "ink"),
  card("merchant", 990, 262, "Merchant", "fulfils on Settled", "ink"),
];

const flow = (id: string, source: string, target: string, opts: Partial<Edge> = {}): Edge => ({
  id, source, target, type: "smoothstep", animated: true,
  markerEnd: { type: MarkerType.ArrowClosed, width: 14, height: 14 },
  ...opts,
});

const edges: Edge[] = [
  flow("mandate", "wallet", "sig", { sourceHandle: "r", targetHandle: "l", label: "mandate", animated: false }),
  flow("cart", "agent", "sig", { sourceHandle: "r", targetHandle: "l", label: "cart", animated: false }),
  flow("g1", "sig", "scope", { sourceHandle: "b", targetHandle: "t", animated: false }),
  flow("g2", "scope", "budget", { sourceHandle: "b", targetHandle: "t", animated: false }),
  flow("g3", "budget", "authorized", { sourceHandle: "b", targetHandle: "t", animated: false }),
  flow("l1", "authorized", "paid", { sourceHandle: "r", targetHandle: "l" }),
  flow("l2", "paid", "settled", { sourceHandle: "r", targetHandle: "l" }),
  flow("l3", "settled", "delivered", { sourceHandle: "r", targetHandle: "l" }),
  flow("proof", "paid", "rail", { sourceHandle: "b", targetHandle: "l", label: "proof" }),
  flow("finality", "rail", "settled", { sourceHandle: "r", targetHandle: "b", label: "finality" }),
  flow("deliver", "delivered", "merchant", { sourceHandle: "r", targetHandle: "l" }),
  flow("deny", "scope", "agent", { sourceHandle: "l", targetHandle: "t", label: "SCOPE_MERCHANT_MISMATCH", className: "rf-deny", animated: false }),
];

export default function Architecture() {
  return (
    <div className="arch" aria-label="Architecture: wallet and agent feed the ledger gate; the lifecycle runs through the rail to the merchant; a cart outside scope is refused">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.08 }}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={false}
        panOnDrag={false}
        zoomOnScroll={false}
        zoomOnPinch={false}
        zoomOnDoubleClick={false}
        preventScrolling={false}
        minZoom={0.2}
      />
    </div>
  );
}
