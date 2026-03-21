import React, { useState, useRef, useEffect } from 'react'
import { Button, Card, App, Row, Col, Tooltip, Typography, Input, Segmented, DatePicker } from 'antd';
import { invoke } from '@tauri-apps/api/core'
import { ReloadOutlined, DownloadOutlined, SearchOutlined } from '@ant-design/icons';
import SearchTable from '../../components/SearchTable'
import dayjs, { Dayjs } from 'dayjs';
import 'dayjs/locale/zh-cn';
import { ClassroomTask } from '../../downloadManager';
import { useConfig } from '../../context/ConfigContext';
import { useDownloadManager, useDownloadList } from '../../context/DownloadContext';
import { useAddDownloadTasks } from '../../hooks/useAddDownloadTasks';
import { Subject } from '../../model';
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

  let myRightColumns = rightColumns.map((item) => {
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
            <Button
              icon={<DownloadOutlined />}
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
            rowKey={selectedCourseRange === 'my' ? 'sub_id' : 'course_id'}
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
                  {rightSubList && rightSubList.length !== 0 && <Text ellipsis={{ rows: 1, expandable: false, tooltip: true }} style={{ width: 'calc(100% - 80px)' }}>
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