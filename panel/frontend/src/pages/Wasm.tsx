import { useState, useEffect } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Card, Button, Input, Select, Table, Modal, Form, message, Upload, Tag, Divider, Space, Tabs, Badge } from "antd";
import { UploadOutlined, DeleteOutlined, PlayCircleOutlined, StopOutlined, ReloadOutlined, SettingOutlined, EyeOutlined, DownloadOutlined, CodeOutlined } from "@ant-design/icons";
import { usePageTitle } from "@/hooks/usePageTitle";
import { useAuth } from "@/contexts/AuthContext";
import type { ColumnsType } from "antd/es/table";
import { formatDistanceToNow } from "date-fns";
import { formatBytes } from "@/utils/format";

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
  const { t } = useAuth();
  usePageTitle(t("wasm.title"));
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
        message.error(t("wasm.fetchError"));
      }
    } catch {
      message.error(t("wasm.fetchError"));
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
        message.success(t("wasm.uploadSuccess"));
        fetchComponents();
        setModalVisible(false);
      } else {
        const err = await res.json();
        message.error(err.error || t("wasm.uploadError"));
      }
    } catch {
      message.error(t("wasm.uploadError"));
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
        message.success(t("wasm.execSuccess"));
      } else {
        message.error(data.error || t("wasm.execError"));
      }
    } catch {
      message.error(t("wasm.execError"));
    } finally {
      setExecLoading(false);
    }
  };

  const handleDelete = async (id: string) => {
    if (!window.confirm(t("wasm.confirmDelete"))) return;
    try {
      const res = await fetch(`/api/wasm/components/${id}`, {
        method: "DELETE",
        headers: { Authorization: `Bearer ${localStorage.getItem("token")}` },
      });
      if (res.ok) {
        message.success(t("wasm.deleteSuccess"));
        fetchComponents();
      } else {
        message.error(t("wasm.deleteError"));
      }
    } catch {
      message.error(t("wasm.deleteError"));
    }
  };

  const columns: ColumnsType<WasmComponent> = [
    {
      title: t("wasm.columns.name"),
      dataIndex: "name",
      key: "name",
      width: 150,
    },
    {
      title: t("wasm.columns.version"),
      dataIndex: "version",
      key: "version",
      width: 100,
    },
    {
      title: t("wasm.columns.description"),
      dataIndex: "description",
      key: "description",
      ellipsis: true,
      width: 200,
    },
    {
      title: t("wasm.columns.memory"),
      dataIndex: "resource_limits",
      key: "memory",
      width: 100,
      render: (limits: WasmResourceLimits) => `${limits.max_memory_mb} MB`,
    },
    {
      title: t("wasm.columns.fuel"),
      dataIndex: "resource_limits",
      key: "fuel",
      width: 100,
      render: (limits: WasmResourceLimits) => limits.max_fuel.toLocaleString(),
    },
    {
      title: t("wasm.columns.created"),
      dataIndex: "created_at",
      key: "created_at",
      width: 150,
      render: (date: string) => formatDistanceToNow(new Date(date), { addSuffix: true }),
    },
    {
      title: t("wasm.columns.actions"),
      key: "actions",
      width: 200,
      render: (_: any, record: WasmComponent) => (
        <Space>
          <Button type="link" icon={<PlayCircleOutlined />} onClick={() => openExecModal(record)}>
            {t("wasm.execute")}
          </Button>
          <Button type="link" icon={<SettingOutlined />} onClick={() => openEditModal(record)}>
            {t("wasm.edit")}
          </Button>
          <Button type="link" danger icon={<DeleteOutlined />} onClick={() => handleDelete(record.id)}>
            {t("wasm.delete")}
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
            message.success(t("wasm.uploadSuccess"));
            fetchComponents();
            setModalVisible(false);
          } else {
            message.error(data.error || t("wasm.uploadError"));
          }
        })
        .catch(() => message.error(t("wasm.uploadError")));
    }
  };

  return (
    <div className="wasm-page">
      <div className="page-header">
        <h1>{t("wasm.title")}</h1>
        <p className="description">{t("wasm.description")}</p>
      </div>

      <Tabs activeKey={activeTab} onChange={setActiveTab}>
        <TabPane tab={t("wasm.tabs.list")} key="list">
          <Card>
            <div className="card-actions">
              <Button type="primary" icon={<UploadOutlined />} onClick={() => setModalVisible(true)}>
                {t("wasm.uploadNew")}
              </Button>
              <Button icon={<ReloadOutlined />} onClick={fetchComponents}>
                {t("wasm.refresh")}
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
        <TabPane tab={t("wasm.tabs.resources")} key="resources">
          <Card>
            <h3>{t("wasm.defaultLimits")}</h3>
            <DefaultResourceLimits />
          </Card>
        </TabPane>
      </Tabs>

      {/* Upload Modal */}
      <Modal
        title={t("wasm.uploadNew")}
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
          <Form.Item name="name" label={t("wasm.fields.name")} rules={[{ required: true }]}>
            <Input placeholder={t("wasm.fields.namePlaceholder")} />
          </Form.Item>
          <Form.Item name="version" label={t("wasm.fields.version")} rules={[{ required: true }]}>
            <Input placeholder="1.0.0" />
          </Form.Item>
          <Form.Item name="description" label={t("wasm.fields.description")}>
            <Input.TextArea rows={3} placeholder={t("wasm.fields.descriptionPlaceholder")} />
          </Form.Item>
          <Form.Item name="wit" label={t("wasm.fields.wit")}>
            <Input.TextArea rows={5} placeholder={t("wasm.fields.witPlaceholder")} />
          </Form.Item>
          <Divider>{t("wasm.resourceLimits")}</Divider>
          <Form.Item name="resource_limits.max_memory_mb" label={t("wasm.fields.maxMemory")}>
            <Input type="number" min={1} max={1024} defaultValue={64} style={{ width: 100 }} />
          </Form.Item>
          <Form.Item name="resource_limits.max_fuel" label={t("wasm.fields.maxFuel")}>
            <Input type="number" min={1000} defaultValue={10000000} style={{ width: 150 }} />
          </Form.Item>
          <Form.Item name="resource_limits.epoch_deadline" label={t("wasm.fields.epochDeadline")}>
            <Input type="number" min={1} defaultValue={1000} style={{ width: 100 }} />
          </Form.Item>
          <Form.Item name="resource_limits.max_table_elements" label={t("wasm.fields.maxTableElements")}>
            <Input type="number" min={1} defaultValue={1024} style={{ width: 100 }} />
          </Form.Item>
          <Form.Item name="resource_limits.allowed_hosts" label={t("wasm.fields.allowedHosts")}>
            <Input placeholder={t("wasm.fields.allowedHostsPlaceholder")} />
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
              {t("wasm.dragDrop")} <span>{t("wasm.orClick")}</span>
            </p>
            <p className="ant-upload-hint">
              {t("wasm.wasmOnly")}
            </p>
          </Dragger>
        </Form>
      </Modal>

      {/* Execution Modal */}
      <Modal
        title={`${t("wasm.execute")} - {execComponent?.name}`}
        open={execModalVisible}
        onCancel={() => setExecModalVisible(false)}
        footer={null}
        width={700}
      >
        <div className="exec-modal">
          <div className="exec-args">
            <label>{t("wasm.execArgs")}</label>
            <Input.TextArea
              rows={3}
              value={execArgs}
              onChange={(e) => setExecArgs(e.target.value)}
              placeholder={t("wasm.execArgsPlaceholder")}
            />
            <Space>
              <Button type="primary" loading={execLoading} icon={<PlayCircleOutlined />} onClick={handleExecute}>
                {t("wasm.execute")}
              </Button>
              <Button onClick={() => setExecModalVisible(false)}>{t("common.cancel")}</Button>
            </Space>
          </div>
          {execResult && (
            <Divider />
            <div className="exec-result">
              <h4>{t("wasm.result")}</h4>
              <div className="result-meta">
                <Badge status={execResult.success ? "success" : "error"}>
                  {execResult.success ? t("wasm.success") : t("wasm.failed")}
                </Badge>
                <span>{t("wasm.exitCode")}: {execResult.exit_code}</span>
                <span>{t("wasm.executionTime")}: {execResult.execution_time_ms}ms</span>
                <span>{t("wasm.fuelConsumed")}: {execResult.fuel_consumed.toLocaleString()}</span>
              </div>
              <div className="result-output">
                <h5>{t("wasm.stdout")}</h5>
                <pre>{execResult.stdout || t("wasm.empty")}</pre>
                <h5>{t("wasm.stderr")}</h5>
                <pre>{execResult.stderr || t("wasm.empty")}</pre>
              </div>
            </div>
          )}
        </div>
      </Modal>
    </div>
  );
};

const DefaultResourceLimits: React.FC = () => {
  const { t } = useAuth();
  return (
    <div className="default-limits">
      <Table
        columns={[
          { title: t("wasm.fields.maxMemory"), dataIndex: "key", render: () => "64 MB" },
          { title: t("wasm.fields.maxFuel"), dataIndex: "key", render: () => "10,000,000" },
          { title: t("wasm.fields.epochDeadline"), dataIndex: "key", render: () => "1000" },
          { title: t("wasm.fields.maxTableElements"), dataIndex: "key", render: () => "1024" },
          { title: t("wasm.fields.allowedHosts"), dataIndex: "key", render: () => t("wasm.none") },
        ]}
        dataSource={[{ key: "default" }]}
      />
    </div>
  );
};

export default Wasm;