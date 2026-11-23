import { useState, useEffect } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Card, Button, Input, Select, Table, Modal, Form, message, Upload, Tag, Divider, Space, Tabs, Badge } from "antd";
import { UploadOutlined, DeleteOutlined, PlayCircleOutlined, StopOutlined, ReloadOutlined, SettingOutlined, EyeOutlined, DownloadOutlined, CodeOutlined } from "@ant-design/icons";
import { usePageTitle } from "@/hooks/usePageTitle";
import { useAuth } from "@/context/AuthContext";
import type { ColumnsType } from "antd/es/table";
import { formatDistanceToNow } from "date-fns";
import { formatSize } from "@/utils/format";

const { Option } = Select;
const { TabPane } = Tabs;
const { Dragger } = Upload;

interface WasmComponent {
  id: string;
  name: string;
  version: string;
  description: string;
  component_path: string;
  wit_path?: string;
  resource_limits: WasmResourceLimits;
  created_at: string;
  updated_at: string;
}

interface WasmResourceLimits {
  max_memory_mb: number;
  max_fuel: number;
  epoch_deadline: number;
  max_table_elements: number;
  allowed_hosts: string[];
}

interface WasmExecutionResult {
  success: boolean;
  exit_code: number;
  stdout: string;
  stderr: string;
  execution_time_ms: number;
  fuel_consumed: number;
}

const Wasm: React.FC = () => {
  const navigate = useNavigate();
    usePageTitle("WASM Components");
  const [components, setComponents] = useState<WasmComponent[]>([]);
  const [loading, setLoading] = useState(false);
  const [modalVisible, setModalVisible] = useState(false);
  const [execModalVisible, setExecModalVisible] = useState(false);
  const [editingComponent, setEditingComponent] = useState<WasmComponent | null>(null);
  const [execComponent, setExecComponent] = useState<WasmComponent | null>(null);
  const [formData, setFormData] = useState<Partial<WasmComponent>>({
    name: "",
    version: "",
    description: "",
    resource_limits: {
      max_memory_mb: 64,
      max_fuel: 10_000_000,
      epoch_deadline: 1000,
      max_table_elements: 1024,
      allowed_hosts: [],
    },
    wit: "",
  });
  const [execArgs, setExecArgs] = useState("");
  const [execResult, setExecResult] = useState<WasmExecutionResult | null>(null);
  const [execLoading, setExecLoading] = useState(false);
  const [activeTab, setActiveTab] = useState("list");

  const fetchComponents = async () => {
    setLoading(true);
    try {
      const res = await fetch("/api/wasm/components", {
        headers: { Authorization: `Bearer ${localStorage.getItem("token")}` },
      });
      if (res.ok) {
        const data = await res.json();
        setComponents(data);
      } else {
        message.error("Failed to fetch WASM components");
      }
    } catch {
      message.error("Failed to fetch WASM components");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchComponents();
  }, []);

  const handleUpload = async (file: File) => {
    const formData = new FormData();
    formData.append("name", formData.get("name") || "");
    formData.append("version", formData.get("version") || "");
    formData.append("description", formData.get("description") || "");
    formData.append("component", file);
    if (formData.get("wit")) {
      formData.append("wit", formData.get("wit")!);
    }
    formData.append("resource_limits", JSON.stringify(formData.get("resource_limits")));

    try {
      const res = await fetch("/api/wasm/upload", {
        method: "POST",
        headers: { Authorization: `Bearer ${localStorage.getItem("token")}` },
        body: formData,
      });
      if (res.ok) {
        message.success("WASM component uploaded successfully");
        fetchComponents();
        setModalVisible(false);
      } else {
        const err = await res.json();
        message.error(err.error || "Failed to upload WASM component");
      }
    } catch {
      message.error("Failed to upload WASM component");
    }
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    // This is handled by the upload component
  };

  const handleExecute = async () => {
    if (!execComponent) return;
    setExecLoading(true);
    setExecResult(null);
    try {
      const res = await fetch("/api/wasm/execute", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${localStorage.getItem("token")}`,
        },
        body: JSON.stringify({
          component_id: execComponent.id,
          args: execArgs.split(" ").filter((a) => a.trim()),
        }),
      });
      const data = await res.json();
      if (res.ok) {
        setExecResult(data);
        message.success("WASM component executed successfully");
      } else {
        message.error(data.error || "Failed to execute WASM component");
      }
    } catch {
      message.error("Failed to execute WASM component");
    } finally {
      setExecLoading(false);
    }
  };

  const handleDelete = async (id: string) => {
    if (!window.confirm("wasm.confirmDelete")) return;
    try {
      const res = await fetch(`/api/wasm/components/${id}`, {
        method: "DELETE",
        headers: { Authorization: `Bearer ${localStorage.getItem("token")}` },
      });
      if (res.ok) {
        message.success("wasm.deleteSuccess");
        fetchComponents();
      } else {
        message.error("wasm.deleteError");
      }
    } catch {
      message.error("wasm.deleteError");
    }
  };

  const columns: ColumnsType<WasmComponent> = [
    {
      title: "wasm.columns.name",
      dataIndex: "name",
      key: "name",
      width: 150,
    },
    {
      title: "wasm.columns.version",
      dataIndex: "version",
      key: "version",
      width: 100,
    },
    {
      title: "wasm.columns.description",
      dataIndex: "description",
      key: "description",
      ellipsis: true,
      width: 200,
    },
    {
      title: "wasm.columns.memory",
      dataIndex: "resource_limits",
      key: "memory",
      width: 100,
      render: (limits: WasmResourceLimits) => `${limits.max_memory_mb} MB`,
    },
    {
      title: "wasm.columns.fuel",
      dataIndex: "resource_limits",
      key: "fuel",
      width: 100,
      render: (limits: WasmResourceLimits) => limits.max_fuel.toLocaleString(),
    },
    {
      title: "wasm.columns.created",
      dataIndex: "created_at",
      key: "created_at",
      width: 150,
      render: (date: string) => formatDistanceToNow(new Date(date), { addSuffix: true }),
    },
    {
      title: "wasm.columns.actions",
      key: "actions",
      width: 200,
      render: (_: any, record: WasmComponent) => (
        <Space>
          <Button type="link" icon={<PlayCircleOutlined />} onClick={() => openExecModal(record)}>
            {"Execute"}
          </Button>
          <Button type="link" icon={<SettingOutlined />} onClick={() => openEditModal(record)}>
            {"wasm.edit"}
          </Button>
          <Button type="link" danger icon={<DeleteOutlined />} onClick={() => handleDelete(record.id)}>
            {"wasm.delete"}
          </Button>
        </Space>
      ),
    },
  ];

  const openEditModal = (component: WasmComponent) => {
    setEditingComponent(component);
    setFormData({
      name: component.name,
      version: component.version,
      description: component.description,
      resource_limits: component.resource_limits,
      wit: component.wit_path ? "" : "",
    });
    setModalVisible(true);
  };

  const openExecModal = (component: WasmComponent) => {
    setExecComponent(component);
    setExecArgs("");
    setExecResult(null);
    setExecModalVisible(true);
  };

  const handleFileChange = (info: any) => {
    if (info.file.status === "done") {
      const file = info.file.originFileObj as File;
      const formData = new FormData();
      formData.append("name", formData.get("name") || "");
      formData.append("version", formData.get("version") || "");
      formData.append("description", formData.get("description") || "");
      formData.append("component", file);
      if (formData.get("wit")) {
        formData.append("wit", formData.get("wit")!);
      }
      formData.append("resource_limits", JSON.stringify(formData.get("resource_limits")));

      fetch("/api/wasm/upload", {
        method: "POST",
        headers: { Authorization: `Bearer ${localStorage.getItem("token")}` },
        body: formData,
      })
        .then((res) => res.json())
        .then((data) => {
          if (data.id) {
            message.success("WASM component uploaded successfully");
            fetchComponents();
            setModalVisible(false);
          } else {
            message.error(data.error || "Failed to upload WASM component");
          }
        })
        .catch(() => message.error("Failed to upload WASM component"));
    }
  };

  return (
    <div className="wasm-page">
      <div className="page-header">
        <h1>{"WASM Components"}</h1>
        <p className="description">{"wasm.description"}</p>
      </div>

      <Tabs activeKey={activeTab} onChange={setActiveTab}>
        <TabPane tab={"wasm.tabs.list"} key="list">
          <Card>
            <div className="card-actions">
              <Button type="primary" icon={<UploadOutlined />} onClick={() => setModalVisible(true)}>
                {"wasm.uploadNew"}
              </Button>
              <Button icon={<ReloadOutlined />} onClick={fetchComponents}>
                {"wasm.refresh"}
              </Button>
            </div>
            <Table
              columns={columns}
              dataSource={components}
              loading={loading}
              rowKey="id"
              pagination={{ pageSize: 20 }}
            />
          </Card>
        </TabPane>
        <TabPane tab={"wasm.tabs.resources"} key="resources">
          <Card>
            <h3>{"wasm.defaultLimits"}</h3>
            <DefaultResourceLimits />
          </Card>
        </TabPane>
      </Tabs>

      {/* Upload Modal */}
      <Modal
        title={"wasm.uploadNew"}
        open={modalVisible}
        onCancel={() => setModalVisible(false)}
        footer={null}
        width={600}
      >
        <Form
          layout="vertical"
          onFinish={(values) => {
            // Handled by upload component
          }}
        >
          <Form.Item name="name" label={"wasm.fields.name"} rules={[{ required: true }]}>
            <Input placeholder={"wasm.fields.namePlaceholder"} />
          </Form.Item>
          <Form.Item name="version" label={"wasm.fields.version"} rules={[{ required: true }]}>
            <Input placeholder="1.0.0" />
          </Form.Item>
          <Form.Item name="description" label={"wasm.fields.description"}>
            <Input.TextArea rows={3} placeholder={"wasm.fields.descriptionPlaceholder"} />
          </Form.Item>
          <Form.Item name="wit" label={"wasm.fields.wit"}>
            <Input.TextArea rows={5} placeholder={"wasm.fields.witPlaceholder"} />
          </Form.Item>
          <Divider>{"wasm.resourceLimits"}</Divider>
          <Form.Item name="resource_limits.max_memory_mb" label={"Max Memory (MB)"}>
            <Input type="number" min={1} max={1024} defaultValue={64} style={{ width: 100 }} />
          </Form.Item>
          <Form.Item name="resource_limits.max_fuel" label={"Max Fuel"}>
            <Input type="number" min={1000} defaultValue={10000000} style={{ width: 150 }} />
          </Form.Item>
          <Form.Item name="resource_limits.epoch_deadline" label={"Epoch Deadline"}>
            <Input type="number" min={1} defaultValue={1000} style={{ width: 100 }} />
          </Form.Item>
          <Form.Item name="resource_limits.max_table_elements" label={"Max Table Elements"}>
            <Input type="number" min={1} defaultValue={1024} style={{ width: 100 }} />
          </Form.Item>
          <Form.Item name="resource_limits.allowed_hosts" label={"Allowed Hosts"}>
            <Input placeholder={"wasm.fields.allowedHostsPlaceholder"} />
          </Form.Item>
          <Divider />
          <Dragger
            onChange={handleFileChange}
            accept=".wasm"
            multiple={false}
          >
            <p className="ant-upload-drag-icon">
              <UploadOutlined />
            </p>
            <p className="ant-upload-text">
              {"wasm.dragDrop"} <span>{"wasm.orClick"}</span>
            </p>
            <p className="ant-upload-hint">
              {"WASM components only"}
            </p>
          </Dragger>
        </Form>
      </Modal>

      {/* Execution Modal */}
      <Modal
        title={`${"Execute"} - {execComponent?.name}`}
        open={execModalVisible}
        onCancel={() => setExecModalVisible(false)}
        footer={null}
        width={700}
      >
        <div className="exec-modal">
          <div className="exec-args">
            <label>{"Execution Arguments"}</label>
            <Input.TextArea
              rows={3}
              value={execArgs}
              onChange={(e) => setExecArgs(e.target.value)}
              placeholder={"Enter command-line arguments for the WASM component"}
            />
            <Space>
              <Button type="primary" loading={execLoading} icon={<PlayCircleOutlined />} onClick={handleExecute}>
                {"Execute"}
              </Button>
              <Button onClick={() => setExecModalVisible(false)}>{"Cancel"}</Button>
            </Space>
          </div>
          {execResult && (
            <>
              <Divider />
              <div className="exec-result">
              <h4>{"Result"}</h4>
              <div className="result-meta">
                <Badge status={execResult.success ? "success" : "error"}>
                  {execResult.success ? "Success" : "Failed"}
                </Badge>
                <span>{"Exit Code"}: {execResult.exit_code}</span>
                <span>{"Execution Time"}: {execResult.execution_time_ms}ms</span>
                <span>{"Fuel Consumed"}: {execResult.fuel_consumed.toLocaleString()}</span>
              </div>
              <div className="result-output">
                <h5>{"Standard Output"}</h5>
                <pre>{execResult.stdout || "(empty)"}</pre>
                <h5>{"Standard Error"}</h5>
                <pre>{execResult.stderr || "(empty)"}</pre>
              </div>
            </div>
            </>
          )}
        </div>
      </Modal>
    </div>
  );
};

const DefaultResourceLimits: React.FC = () => {
    return (
    <div className="default-limits">
      <Table
        columns={[
          { title: "Max Memory (MB)", dataIndex: "key", render: () => "64 MB" },
          { title: "Max Fuel", dataIndex: "key", render: () => "10,000,000" },
          { title: "Epoch Deadline", dataIndex: "key", render: () => "1000" },
          { title: "Max Table Elements", dataIndex: "key", render: () => "1024" },
          { title: "Allowed Hosts", dataIndex: "key", render: () => "None" },
        ]}
        dataSource={[{ key: "default" }]}
      />
    </div>
  );
};

export default Wasm;