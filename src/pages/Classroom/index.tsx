import React, { useState, useRef, useEffect } from 'react'
import { Button, Card, App, Row, Col, Tooltip, Typography, Input, Segmented, DatePicker, Dropdown } from 'antd';
import { invoke } from '@tauri-apps/api/core'
import { ReloadOutlined, DownloadOutlined, SearchOutlined, PlayCircleOutlined, StopOutlined, ExportOutlined, HistoryOutlined, DownOutlined } from '@ant-design/icons';
import SearchTable from '../../components/SearchTable'
import dayjs, { Dayjs } from 'dayjs';
import 'dayjs/locale/zh-cn';
import { ClassroomTask } from '../../downloadManager';
import { useConfig } from '../../context/ConfigContext';
import { useDownloadManager, useDownloadList } from '../../context/DownloadContext';
import { useAddDownloadTasks } from '../../hooks/useAddDownloadTasks';
import { LiveTranscriptSessionStatus, Subject } from '../../model';
import { ColumnType } from 'antd/es/table';

dayjs.locale('zh-cn')

const { Text } = Typography
const { RangePicker } = DatePicker;

export default function Classroom() {

  const { notification } = App.useApp()
  const { config } = useConfig();
  const downloadManager = useDownloadManager();
  const addDownloadTasks = useAddDownloadTasks();
  const toPdf = config.to_pdf;

  const [selectedDateMethod, setSelectedDateMethod] = useState<'day' | 'week' | 'month'>('week')
  const [selectedCourseRange, setSelectedCourseRange] = useState<'my' | 'all'>('my')
  const [leftSubList, setLeftSubList] = useState<Subject[]>([])
  const [rightSubList, setRightSubList] = useState<Subject[]>([])
  const [selectedLeftKeys, setSelectedLeftKeys] = useState<React.Key[]>([])
  const [selectedRightKeys, setSelectedRightKeys] = useState<React.Key[]>([])
  const [loadingLeftSubList, setLoadingLeftSubList] = useState(false)
  const [loadingRightSubList, setLoadingRightSubList] = useState(false)
  const [searchCourseName, setSearchCourseName] = useState('')
  const [searchTeacherName, setSearchTeacherName] = useState('')
  const startAt = useRef(dayjs().startOf('week').format('YYYY-MM-DD'))
  const endAt = useRef(dayjs().endOf('week').format('YYYY-MM-DD'))
  const [dayRange, setDayRange] = useState<[Dayjs, Dayjs]>([dayjs(), dayjs()])
  const [weekValue, setWeekValue] = useState<Dayjs>(dayjs())
  const [monthValue, setMonthValue] = useState<Dayjs>(dayjs())
  const [liveSessions, setLiveSessions] = useState<LiveTranscriptSessionStatus[]>([])
  const [liveBusy, setLiveBusy] = useState(false)
  const autoStartTriggeredRef = useRef<Set<number>>(new Set())

  const selectDateMethodOptions = [
    { label: '日', value: 'day' },
    { label: '周', value: 'week' },
    { label: '月', value: 'month' },
  ]

  const selectCourseRangeOptions = [
    { label: '我的课程', value: 'my' },
    { label: '全部课程', value: 'all' }
  ]

  const changeDateMethod = (value: 'day' | 'week' | 'month') => {
    setSelectedDateMethod(value)
    if (value === 'day') {
      startAt.current = dayjs().format('YYYY-MM-DD')
      endAt.current = dayjs().format('YYYY-MM-DD')
    } else if (value === 'week') {
      startAt.current = dayjs().startOf('week').format('YYYY-MM-DD')
      endAt.current = dayjs().endOf('week').format('YYYY-MM-DD')
    } else {
      startAt.current = dayjs().startOf('month').format('YYYY-MM-DD')
      endAt.current = dayjs().endOf('month').format('YYYY-MM-DD')
    }
    updateMySubList()
  }

  const changeDateRange = (value: any) => {
    if (selectedDateMethod === 'day') {
      setDayRange(value)
      startAt.current = value[0].format('YYYY-MM-DD')
      endAt.current = value[1].format('YYYY-MM-DD')
    } else if (selectedDateMethod === 'week') {
      setWeekValue(value)
      startAt.current = value.startOf('week').format('YYYY-MM-DD')
      endAt.current = value.endOf('week').format('YYYY-MM-DD')
    } else {
      setMonthValue(value)
      startAt.current = value.startOf('month').format('YYYY-MM-DD')
      endAt.current = value.endOf('month').format('YYYY-MM-DD')
    }
    updateMySubList()
  }

  const updateMySubList = () => {
    setLoadingRightSubList(true)
    invoke<Subject[]>('get_range_subs', { startAt: startAt.current, endAt: endAt.current }).then((res) => {
      setRightSubList(res)
      setSelectedRightKeys(res.map((item) => item.sub_id))
    }).catch((err) => {
      notification.error({
        message: '获取课程列表失败',
        description: String(err)
      })
    }).finally(() => {
      setLoadingRightSubList(false)
    })
  }

  const updateAllSubList = () => {
    let subs = leftSubList.filter((item) => selectedLeftKeys.includes(item.course_id))
    if (subs.length === 0) {
      notification.error({
        message: '请选择课程',
      })
      return
    }
    let course_ids = subs.map((item) => item.course_id)
    setLoadingRightSubList(true)
    invoke<Subject[]>('get_course_all_sub_ppts', { courseIds: course_ids }).then((res) => {
      const subs = res.filter((item) => item.ppt_image_urls.length !== 0)
      if (subs.length === 0) {
        notification.error({
          message: '没有发现智云 PPT',
        })
      }
      setRightSubList(subs)
      setSelectedRightKeys(subs.map((item) => item.sub_id))
    }).catch((err) => {
      notification.error({
        message: '获取课件列表失败',
        description: String(err)
      })
    }).finally(() => {
      setLoadingRightSubList(false)
    })
  }

  useEffect(() => {
    updateMySubList()
  }, [])

  const refreshLiveSessions = async () => {
    if (!config.show_live_capture_controls) {
      setLiveSessions([])
      return
    }
    try {
      const sessions = await invoke<LiveTranscriptSessionStatus[]>('get_live_transcript_sessions')
      setLiveSessions(sessions)
    } catch (err) {
      notification.error({
        message: '获取同传会话状态失败',
        description: String(err),
      })
    }
  }

  useEffect(() => {
    if (!config.show_live_capture_controls) {
      setLiveSessions([])
      return
    }
    refreshLiveSessions()
    const timer = window.setInterval(() => {
      refreshLiveSessions()
    }, 4000)
    return () => window.clearInterval(timer)
  }, [config.show_live_capture_controls])

  useEffect(() => {
    if (!config.show_live_capture_controls || !config.live_capture_auto_start) {
      if (config.show_live_capture_controls && !config.live_capture_auto_start) {
        rightSubList.forEach((sub) => {
          if (sub.sub_id > 0) {
            invoke('cancel_scheduled_live_transcript_capture', {
              subId: sub.sub_id,
            }).catch(() => { })
            autoStartTriggeredRef.current.delete(sub.sub_id)
          }
        })
      }
      return
    }
    const scheduleInBackend = async () => {
      for (const sub of rightSubList) {
        if (!sub.start_at || sub.sub_id <= 0) {
          continue
        }
        if (autoStartTriggeredRef.current.has(sub.sub_id)) {
          continue
        }
        autoStartTriggeredRef.current.add(sub.sub_id)
        try {
          await invoke('schedule_live_transcript_capture_at', {
            courseId: sub.course_id,
            subId: sub.sub_id,
            startAt: sub.start_at,
          })
        } catch (err) {
          autoStartTriggeredRef.current.delete(sub.sub_id)
          notification.error({
            message: '自动采集调度失败',
            description: `${sub.course_name} - ${sub.sub_name}: ${String(err)}`,
          })
        }
      }
    }
    scheduleInBackend()
  }, [config.show_live_capture_controls, config.live_capture_auto_start, rightSubList])

  const selectedLiveSubs = rightSubList.filter((item) => selectedRightKeys.includes(item.sub_id))

  const startLiveCapture = async () => {
    if (selectedLiveSubs.length === 0) {
      notification.error({ message: '请选择至少一节课' })
      return
    }
    setLiveBusy(true)
    const failures: string[] = []
    for (const sub of selectedLiveSubs) {
      try {
        await invoke('start_live_transcript_capture', {
          courseId: sub.course_id,
          subId: sub.sub_id,
        })
      } catch (err) {
        failures.push(`${sub.sub_name}: ${String(err)}`)
      }
    }
    await refreshLiveSessions()
    setLiveBusy(false)
    if (failures.length > 0) {
      notification.error({
        message: '部分会话启动失败',
        description: failures.join('\n'),
      })
    } else {
      notification.success({ message: '已开始直播同传采集' })
    }
  }

  const stopLiveCapture = async () => {
    if (selectedRightKeys.length === 0) {
      notification.error({ message: '请选择至少一节课' })
      return
    }
    setLiveBusy(true)
    const failures: string[] = []
    for (const subId of selectedRightKeys.map((k) => Number(k))) {
      try {
        await invoke('stop_live_transcript_capture', { subId })
      } catch (err) {
        failures.push(`${subId}: ${String(err)}`)
      }
    }
    await refreshLiveSessions()
    setLiveBusy(false)
    if (failures.length > 0) {
      notification.error({
        message: '部分会话停止失败',
        description: failures.join('\n'),
      })
    } else {
      notification.success({ message: '已停止采集' })
    }
  }

  const backfillLiveHistory = async () => {
    if (selectedRightKeys.length === 0) {
      notification.error({ message: '请选择至少一节课' })
      return
    }
    setLiveBusy(true)
    const resultMessages: string[] = []
    for (const subId of selectedRightKeys.map((k) => Number(k))) {
      try {
        const inserted = await invoke<number>('backfill_live_transcript_from_history', { subId })
        resultMessages.push(`sub_id=${subId} 回填 ${inserted} 行`)
      } catch (err) {
        resultMessages.push(`sub_id=${subId} 失败: ${String(err)}`)
      }
    }
    await refreshLiveSessions()
    setLiveBusy(false)
    notification.info({
      message: '历史字幕回填完成',
      description: resultMessages.join('\n'),
    })
  }

  const exportLiveTranscript = async () => {
    if (selectedLiveSubs.length === 0) {
      notification.error({ message: '请选择至少一节课' })
      return
    }

    setLiveBusy(true)
    for (const sub of selectedLiveSubs) {
      try {
        const savedPath = await invoke<string>('export_live_transcript_to_file', {
          subId: sub.sub_id,
          courseName: sub.course_name,
          subName: sub.sub_name,
          format: 'txt',
          includeOriginal: true,
          withTimestamps: true,
        })
        notification.success({
          message: '导出同传成功',
          description: `已保存到: ${savedPath}`,
        })
      } catch (err) {
        notification.error({
          message: '导出同传失败',
          description: `${sub.sub_name}: ${String(err)}`,
        })
      }
    }
    setLiveBusy(false)
  }

  const handleLiveActionClick = ({ key }: { key: string }) => {
    if (key === 'start') {
      startLiveCapture()
      return
    }
    if (key === 'stop') {
      stopLiveCapture()
      return
    }
    if (key === 'backfill') {
      backfillLiveHistory()
      return
    }
    if (key === 'export') {
      exportLiveTranscript()
    }
  }

  const leftColumns: ColumnType<Subject>[] = [
    { dataIndex: 'course_name', title: '课程名称' },
    { dataIndex: 'sub_name', title: '上课时间' },
    { dataIndex: 'lecturer_name', title: '教师', responsive: ['lg'] },
  ];

  const rightColumns: ColumnType<Subject>[] = [
    {
      title: '课程名称',
      dataIndex: 'course_name',
      sorter: (a, b) => a.course_name.localeCompare(b.course_name),
    },
    {
      dataIndex: 'sub_name',
      title: '上课时间',
      sorter: (a, b) => a.sub_name.localeCompare(b.sub_name),
    },
    {
      dataIndex: 'lecturer_name',
      title: '教师',
      responsive: ['lg'],
    },
    {
      dataIndex: 'ppt_image_urls',
      title: '页数',
      render: (urls: string[]) => urls.length,
      // @ts-ignore
      searchable: false,
      sorter: (a, b) => a.ppt_image_urls.length - b.ppt_image_urls.length,
    }
  ];

  const liveSessionMap = new Map(liveSessions.map((session) => [session.sub_id, session]))
  const rightColumnsWithLive: ColumnType<Subject>[] = config.show_live_capture_controls
    ? [
      ...rightColumns,
      {
        title: '同传采集',
        dataIndex: 'sub_id',
        render: (_: any, row: Subject) => {
          const status = liveSessionMap.get(row.sub_id)
          if (!status) {
            return '未开始'
          }
          return `${status.is_running ? '采集中' : '已停止'} (${status.line_count})`
        },
        // @ts-ignore
        searchable: false,
      }
    ]
    : rightColumns;

  let myRightColumns = rightColumnsWithLive.map((item) => {
    if (item.dataIndex === 'lecturer_name') {
      return { ...item, responsive: undefined }
    }
    return item
  });

  const downloadSubsPPT = () => {
    let subs = rightSubList.filter((item) => selectedRightKeys.includes(item.sub_id))
    if (subs.length === 0) {
      notification.error({
        message: '请选择课件',
      })
      return
    }
    let tasks = subs.map((item) => new ClassroomTask(item, toPdf))
    addDownloadTasks(tasks)
    setRightSubList(rightSubList.filter((item) => !selectedRightKeys.includes(item.sub_id)))
    setSelectedRightKeys([])
  }

  const downloadSubsSubtitle = () => {
    let subs = rightSubList.filter((item) => selectedRightKeys.includes(item.sub_id))
    if (subs.length === 0) {
      notification.error({
        message: '请选择课件',
      })
      return
    }
    let tasks = subs.map((item) => new ClassroomTask(item, false, true))
    addDownloadTasks(tasks)
    setRightSubList(rightSubList.filter((item) => !selectedRightKeys.includes(item.sub_id)))
    setSelectedRightKeys([])
  }

  const searchCourse = () => {
    if (searchCourseName === '' && searchTeacherName === '') {
      notification.error({
        message: '请输入搜索关键字',
      })
      return
    }
    setLoadingLeftSubList(true)
    invoke<Subject[]>('search_courses', { courseName: searchCourseName, teacherName: searchTeacherName }).then((res) => {
      setLeftSubList(res)
      setSelectedLeftKeys([])
    }).catch((err) => {
      notification.error({
        message: '搜索课程失败',
        description: String(err)
      })
    }).finally(() => {
      setLoadingLeftSubList(false)
    })
  }

  return (
    <div style={{ margin: 20 }}>
      <Card styles={{ body: { padding: 15 } }}>
        <div style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
        }} >
          <div style={{ display: 'flex', alignItems: 'center', flexDirection: 'row' }}>
            <Segmented
              options={selectCourseRangeOptions}
              onChange={(value) => {
                setLeftSubList([])
                setSelectedLeftKeys([])
                setRightSubList([])
                setSelectedRightKeys([])
                setSelectedCourseRange(value as 'my' | 'all')
                if (value === 'my') {
                  updateMySubList()
                }
              }}
              value={selectedCourseRange}
              style={{ minWidth: 155 }}
            />
          </div>
          {selectedCourseRange === 'my' &&
            <div style={{ display: 'flex', alignItems: 'center', flexDirection: 'row', marginLeft: 20 }}>
              <Segmented
                options={selectDateMethodOptions}
                onChange={(v) => changeDateMethod(v as 'day' | 'week' | 'month')}
                value={selectedDateMethod}
                style={{ minWidth: 106, marginRight: 20 }}
              />
              {selectedDateMethod === 'day' && <RangePicker
                value={dayRange}
                onChange={changeDateRange}
                disabled={loadingRightSubList}
              />}
              {selectedDateMethod === 'week' && <DatePicker
                value={weekValue}
                picker='week'
                onChange={changeDateRange}
                disabled={loadingRightSubList}
              />}
              {selectedDateMethod === 'month' && <DatePicker
                value={monthValue}
                picker='month'
                onChange={changeDateRange}
                disabled={loadingRightSubList}
              />}
            </div>}
          {selectedCourseRange === 'all' &&
            <div style={{ display: 'flex', alignItems: 'center', flexDirection: 'row', marginLeft: 20 }}>
              <div style={{ display: 'flex', alignItems: 'center', flexDirection: 'row' }}>
                <Input placeholder='课程名称' value={searchCourseName} onChange={(e) => setSearchCourseName(e.target.value)} />
              </div>
              <div style={{ display: 'flex', alignItems: 'center', flexDirection: 'row', marginLeft: 20 }}>
                <Input placeholder='教师名称' value={searchTeacherName} onChange={(e) => setSearchTeacherName(e.target.value)} />
              </div>
              <div style={{ display: 'flex', alignItems: 'center', flexDirection: 'row', marginLeft: 20 }}>
                <Tooltip title='搜索全部课程'>
                  <Button icon={<SearchOutlined />} onClick={searchCourse} loading={loadingLeftSubList} />
                </Tooltip>
              </div>
            </div>
          }
          <div style={{ display: 'flex', alignItems: 'center', flexDirection: 'row', marginLeft: 20 }}>
            {config.show_live_capture_controls && (
              <Dropdown
                trigger={['click']}
                menu={{
                  onClick: handleLiveActionClick,
                  items: [
                    { key: 'start', label: '开始采集', icon: <PlayCircleOutlined /> },
                    { key: 'stop', label: '停止采集', icon: <StopOutlined /> },
                    { key: 'backfill', label: '回填历史', icon: <HistoryOutlined /> },
                    { key: 'export', label: '导出同传', icon: <ExportOutlined /> },
                  ],
                }}
              >
                <Button disabled={loadingRightSubList || liveBusy}>
                  同传操作 <DownOutlined />
                </Button>
              </Dropdown>
            )}
            <Button
              icon={<DownloadOutlined />}
              style={{ marginLeft: config.show_live_capture_controls ? 10 : 0 }}
              onClick={downloadSubsSubtitle}
              disabled={loadingRightSubList || !config.download_subtitle}
            >{'下载ASR'}</Button>
            <Button
              type='primary'
              icon={<DownloadOutlined />}
              style={{ marginLeft: 10 }}
              onClick={downloadSubsPPT}
              disabled={loadingRightSubList}
            >{'下载课件'}</Button>
          </div>
        </div>
      </Card>
      <Row gutter={20} style={{ marginTop: 20 }}>
        {selectedCourseRange === 'all' && <Col xs={10}>
          <SearchTable<Subject>
            rowSelection={{
              selectedRowKeys: selectedLeftKeys,
              onChange: setSelectedLeftKeys,
            }}
            rowKey='course_id'
            // @ts-ignore
            columns={leftColumns}
            dataSource={leftSubList}
            pagination={false}
            scroll={{ y: 'calc(100vh - 270px)' }}
            size='small'
            bordered
            footer={() => ''}
            title={() => `课程列表：已选择 ${selectedLeftKeys.length} 门课程`}
            loading={loadingLeftSubList}
          />
        </Col>}
        <Col xs={selectedCourseRange === 'all' ? 14 : 24}>
          <SearchTable<Subject>
            rowSelection={{
              selectedRowKeys: selectedRightKeys,
              onChange: setSelectedRightKeys,
            }}
            rowKey='sub_id'
            // @ts-ignore
            columns={selectedCourseRange === 'my' ? myRightColumns : rightColumns}
            dataSource={rightSubList}
            pagination={false}
            scroll={{ y: 'calc(100vh - 270px)' }}
            size='small'
            bordered
            footer={() => ''}
            loading={loadingRightSubList}
            title={() => {
              return (
                <>
                  {rightSubList && rightSubList.length !== 0 && <Text ellipsis={{ tooltip: true }} style={{ width: 'calc(100% - 80px)' }}>
                    课件列表：已选择 {selectedRightKeys.length} 个课件 共 {rightSubList.filter((item) => selectedRightKeys.includes(item.sub_id)).reduce((total, item) => {
                      return total + item.ppt_image_urls.length
                    }, 0)} 页</Text>}
                  {(rightSubList && rightSubList.length === 0) && '课件列表为空  点击右侧刷新👉'}
                  <div style={{ float: 'right' }}>
                    <Tooltip title='刷新课件列表'>
                      <Button
                        type='text'
                        size='small'
                        icon={<ReloadOutlined />}
                        onClick={selectedCourseRange === 'my' ? updateMySubList : updateAllSubList}
                        loading={loadingRightSubList}
                      />
                    </Tooltip>
                  </div>
                </>
              )
            }}
          />
        </Col>
      </Row>
    </div>
  )
}