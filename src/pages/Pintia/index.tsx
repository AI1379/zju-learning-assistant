import React, { useEffect, useMemo, useState } from 'react'
import { Button, Card, App, Typography, Table, Tag, Space, Input, Checkbox, Divider, Segmented, Tooltip } from 'antd';
import { SyncOutlined, LoginOutlined, LogoutOutlined, SearchOutlined } from '@ant-design/icons';
import dayjs from 'dayjs';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-shell';

const { Text } = Typography;

interface PintiaSet {
  id: string;
  name: string;
  type: string;
  timeType: string;
  organizationName?: string;
  ownerNickname?: string;
  startAt?: string | null;
  endAt?: string | null;
}

const setTypeInfo = (type: string): { label: string; color: string } => {
  switch (type) {
    case 'HOMEWORK': return { label: '作业', color: 'blue' };
    case 'EXAM': return { label: '考试', color: 'volcano' };
    case 'CONTEST': return { label: '竞赛', color: 'purple' };
    case 'BOOK': return { label: '题库', color: 'cyan' };
    default: return { label: '题集', color: 'default' };
  }
}

const getSetStatus = (set: PintiaSet): { label: string; color: string } => {
  const now = dayjs();
  if (set.timeType === 'ALWAYS_AVAILABLE' || (!set.endAt && !set.startAt)) {
    return { label: '永久开放', color: 'default' };
  }
  if (set.startAt && now.isBefore(dayjs(set.startAt))) {
    return { label: '未开始', color: 'gold' };
  }
  if (set.endAt) {
    if (now.isAfter(dayjs(set.endAt))) {
      return { label: '已截止', color: 'default' };
    }
    return { label: '进行中', color: 'processing' };
  }
  return { label: '进行中', color: 'processing' };
}

export default function Pintia() {
  const { notification } = App.useApp();

  const [account, setAccount] = useState<string | null>(null);
  const [checking, setChecking] = useState(true);
  const [assignments, setAssignments] = useState<PintiaSet[]>([]);
  const [loading, setLoading] = useState(false);
  const [lastSync, setLastSync] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<string>('ongoing');
  const [keyword, setKeyword] = useState('');

  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [remember, setRemember] = useState(true);
  const [loggingIn, setLoggingIn] = useState(false);
  const [cookie, setCookie] = useState('');
  const [cookieLoggingIn, setCookieLoggingIn] = useState(false);

  const refresh = (silent = false) => {
    setLoading(true);
    invoke<PintiaSet[]>('pintia_get_assignments').then((res) => {
      setAssignments(res || []);
      setLastSync(dayjs().format('YYYY-MM-DD HH:mm:ss'));
      if (!silent) notification.success({ message: '拼题A题集同步成功' });
    }).catch((err) => {
      notification.error({ message: '拼题A题集同步失败', description: String(err) });
    }).finally(() => setLoading(false));
  }

  useEffect(() => {
    invoke<string | null>('pintia_check_login').then((res) => {
      setAccount(res);
      if (res) refresh(true);
    }).catch(() => setAccount(null)).finally(() => setChecking(false));
  }, []);

  const handleLogin = () => {
    if (!username || !password) {
      notification.warning({ message: '请输入拼题A账号和密码' });
      return;
    }
    setLoggingIn(true);
    invoke<string>('pintia_login', { username, password, remember }).then((res) => {
      setAccount(res);
      setPassword('');
      notification.success({ message: `拼题A已登录：${res}` });
      refresh(true);
    }).catch((err) => {
      notification.error({ message: '拼题A登录失败', description: String(err) });
    }).finally(() => setLoggingIn(false));
  }

  const handleCookieLogin = () => {
    if (!cookie.trim()) {
      notification.warning({ message: '请粘贴 PTASession Cookie' });
      return;
    }
    setCookieLoggingIn(true);
    invoke<string>('pintia_login_with_cookie', { cookie }).then((res) => {
      setAccount(res);
      setCookie('');
      notification.success({ message: '拼题A Cookie 登录成功' });
      refresh(true);
    }).catch((err) => {
      notification.error({ message: '拼题A Cookie 登录失败', description: String(err) });
    }).finally(() => setCookieLoggingIn(false));
  }

  const handleLogout = () => {
    invoke('pintia_logout').then(() => {
      setAccount(null);
      setAssignments([]);
      notification.success({ message: '拼题A已退出登录' });
    }).catch((err) => {
      notification.error({ message: '拼题A退出登录失败', description: String(err) });
    });
  }

  const filtered = useMemo(() => {
    return assignments.filter((set) => {
      const status = getSetStatus(set).label;
      if (statusFilter === 'ongoing' && status !== '进行中' && status !== '未开始') return false;
      if (statusFilter === 'ended' && status !== '已截止') return false;
      if (keyword) {
        const kw = keyword.toLowerCase();
        if (!set.name.toLowerCase().includes(kw)
          && !(set.organizationName || '').toLowerCase().includes(kw)) return false;
      }
      return true;
    }).sort((a, b) => {
      // ongoing first by nearest deadline, ended last
      const aEnd = a.endAt ? dayjs(a.endAt).valueOf() : Number.MAX_SAFE_INTEGER;
      const bEnd = b.endAt ? dayjs(b.endAt).valueOf() : Number.MAX_SAFE_INTEGER;
      return aEnd - bEnd;
    });
  }, [assignments, statusFilter, keyword]);

  const columns = [
    {
      title: '课程 / 组织',
      dataIndex: 'organizationName',
      key: 'organizationName',
      width: '20%',
      render: (text: string) => text || '拼题A',
    },
    {
      title: '题集名称',
      dataIndex: 'name',
      key: 'name',
      width: '38%',
      render: (text: string, record: PintiaSet) => (
        <a
          onClick={() => {
            open(`https://pintia.cn/problem-sets/${record.id}/exam/problems`).catch((err) => {
              notification.error({ message: '打开链接失败', description: String(err) });
            });
          }}
        >
          {text}
        </a>
      ),
    },
    {
      title: '类型',
      dataIndex: 'type',
      key: 'type',
      width: '8%',
      render: (text: string) => {
        const info = setTypeInfo(text);
        return <Tag color={info.color}>{info.label}</Tag>;
      },
    },
    {
      title: '状态',
      key: 'status',
      width: '9%',
      render: (_: unknown, record: PintiaSet) => {
        const status = getSetStatus(record);
        return <Tag color={status.color}>{status.label}</Tag>;
      },
    },
    {
      title: '开始时间',
      dataIndex: 'startAt',
      key: 'startAt',
      width: '12%',
      render: (text: string | null) => text ? dayjs(text).format('YYYY-MM-DD HH:mm') : '-',
    },
    {
      title: '截止时间',
      dataIndex: 'endAt',
      key: 'endAt',
      width: '13%',
      sorter: (a: PintiaSet, b: PintiaSet) => {
        if (!a.endAt) return 1;
        if (!b.endAt) return -1;
        return dayjs(a.endAt).diff(dayjs(b.endAt));
      },
      render: (text: string | null) => {
        if (!text) {
          return <Tag>无截止时间</Tag>;
        }
        const diff = dayjs(text).diff(dayjs(), 'hour');
        let color = 'green';
        if (diff < 0) color = 'default';
        else if (diff < 24) color = 'red';
        else if (diff < 72) color = 'orange';
        return <Tag color={color}>{dayjs(text).format('YYYY-MM-DD HH:mm')}</Tag>;
      },
    },
  ];

  if (checking) {
    return (
      <div style={{ margin: 20 }}>
        <Card styles={{ body: { padding: 15 } }}>
          <Text type='secondary'>正在检查拼题A登录状态…</Text>
        </Card>
      </div>
    );
  }

  if (!account) {
    return (
      <div style={{ margin: 20, display: 'flex', justifyContent: 'center' }}>
        <Card styles={{ body: { padding: 24 } }} style={{ width: 420 }}>
          <Space direction='vertical' style={{ width: '100%' }} size={12}>
            <Text strong>登录拼题A（PTA）</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              登录后可在此查看 PTA 上的作业、考试与竞赛题集，未截止的作业会自动合并进待办事项。
            </Text>
            <Input
              placeholder='拼题A 邮箱 / 手机号'
              value={username}
              onChange={(e) => setUsername(e.target.value)}
            />
            <Space.Compact style={{ width: '100%' }}>
              <Input.Password
                placeholder='拼题A 密码'
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                onPressEnter={handleLogin}
              />
              <Button type='primary' icon={<LoginOutlined />} loading={loggingIn} onClick={handleLogin}>
                登录
              </Button>
            </Space.Compact>
            <Checkbox checked={remember} onChange={(e) => setRemember(e.target.checked)} style={{ fontSize: 12 }}>
              记住密码并自动登录
            </Checkbox>
            <Divider style={{ margin: '4px 0' }} plain>
              <Text type="secondary" style={{ fontSize: 12 }}>或</Text>
            </Divider>
            <Text type="secondary" style={{ fontSize: 12, display: 'block' }}>
              若登录触发了验证码，可改用 Cookie 登录：在浏览器登录 pintia.cn 后，按 F12 → Network → 任选一个请求 → Request Headers → Cookie 中复制 PTASession= 后面的值。
            </Text>
            <Space.Compact style={{ width: '100%' }}>
              <Input
                placeholder='粘贴 PTASession Cookie 值'
                value={cookie}
                onChange={(e) => setCookie(e.target.value)}
                onPressEnter={handleCookieLogin}
              />
              <Button icon={<LoginOutlined />} loading={cookieLoggingIn} onClick={handleCookieLogin}>
                Cookie 登录
              </Button>
            </Space.Compact>
          </Space>
        </Card>
      </div>
    );
  }

  return (
    <div style={{ margin: 20 }}>
      <Card styles={{ body: { padding: 15 } }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
            <Text>已登录：<Text type="success">{account}</Text></Text>
            <Tooltip title='退出登录'>
              <Button size='small' icon={<LogoutOutlined />} onClick={handleLogout} />
            </Tooltip>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
            <Input
              size='small'
              style={{ width: 180 }}
              prefix={<SearchOutlined />}
              placeholder='搜索题集 / 课程'
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              allowClear
            />
            <Segmented
              value={statusFilter}
              onChange={(value) => setStatusFilter(value as string)}
              options={[
                { label: '未截止', value: 'ongoing' },
                { label: '已截止', value: 'ended' },
                { label: '全部', value: 'all' },
              ]}
            />
            <Button type="primary" size='small' icon={<SyncOutlined />} loading={loading} onClick={() => refresh()}>
              {loading ? '正在同步' : '立即同步'}
            </Button>
          </div>
        </div>
      </Card>
      <Table
        columns={columns}
        dataSource={filtered}
        rowKey={(record) => `${record.id}`}
        pagination={false}
        scroll={{ y: 'calc(100vh - 255px)' }}
        size="small"
        bordered
        footer={() => `最后同步时间：${lastSync ? lastSync : '未同步'}，当前筛选 ${filtered.length} 个题集`}
        style={{ marginTop: 20 }}
        loading={loading}
      />
    </div>
  );
}
